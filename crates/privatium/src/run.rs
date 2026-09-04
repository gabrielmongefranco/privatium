// Project:  Privatium™  |  File: crates/privatium/src/run.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-04  |  Modified: 2026-09-05
// Summary:  Bare `privatium` (spec/cli.md §2) and `privatium dev` (§3): open the node,
//           apply the run's overrides, load every app, bind loopback, and serve
//           `core::handle` until Ctrl-C. `dev` is the same node with the app named — the
//           reloading is the host's own, a stat on the next request (§3, spec/lua-api.md
//           §7), so there is nothing for this file to watch. The weekly snapshots of
//           spec/protocol.md §5 are written by a daily pass here, since a node with no
//           request loop of its own had nobody to write them.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use privatium::adapter;
use privatium_core::store::snapshot;
use privatium_core::{Handler, Mode, Node};

use crate::cli::Global;
use crate::node;

/// What `run` and `dev` pass in.
pub struct Options {
    /// `--port`: for this run, never written to `config.toml`.
    pub port: Option<u16>,
    /// `--solo <slug>`: overrides `[node] mode` for this run (`§2`).
    pub solo: Option<String>,
    /// `--no-discovery`: parses, and is a notice, until Phase 2 has discovery to disable.
    pub no_discovery: bool,
    /// `--open`: a browser on the node — or on the app, under `dev --app`.
    pub open: bool,
    /// `dev --app <slug>`: the app being edited, named in the URL printed and opened.
    pub dev_app: Option<String>,
    /// Whether this is `dev`.
    pub dev: bool,
}

/// How often the node checks whether a snapshot is due (`spec/data-dictionary.md §3.6`,
/// `snapshot.interval_days`, seven by default). Once a day is plenty for a weekly policy
/// and costs a directory listing per app.
const MAINTENANCE_EVERY: Duration = Duration::from_secs(24 * 60 * 60);

pub fn run(global: &Global, options: Options) -> Result<u8> {
    let mut node = node::open(global)?;
    apply_overrides(&mut node, &options)?;
    let report = node.load_apps(&node::roots(&node))?;
    node::print_report(&report, global.verbose || options.dev);

    if options.no_discovery {
        eprintln!(
            "privatium: --no-discovery: there is no discovery to disable in this build — \
             discovery is Phase 2 (docs/roadmap.md); the node listens on loopback only"
        );
    }

    // `dev --app`: the app must be running, and its folder is what the owner edits.
    let mut dev_mount = None;
    if let Some(slug) = &options.dev_app {
        match node.app(slug) {
            Some(app) => {
                eprintln!(
                    "privatium dev: {slug} at {} — a save is served on the next request, \
                     with no restart (spec/cli.md §3)",
                    app.dir()
                        .map_or_else(|| "(no folder)".to_owned(), |dir| dir.display().to_string())
                );
                dev_mount = app.mount().map(str::to_owned);
            }
            None => {
                let why = node::failure_of(&report, slug)
                    .unwrap_or_else(|| "no such app under apps/ (spec/cli.md §3)".to_owned());
                bail!("dev --app {slug}: {why}");
            }
        }
    }

    let port = node.config().node.port;
    let slugs: Vec<String> = report.loaded.clone();
    let handler = Arc::new(Handler::new(node, report));

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async {
            let listener = adapter::bind(port).await.with_context(|| {
                format!("binding 127.0.0.1:{port} — is another node running on that port?")
            })?;
            let addr = listener.local_addr()?;
            print!("{}", adapter::announce(addr));
            let origin = format!("http://{addr}");
            let url = match &dev_mount {
                Some(mount) => format!("{origin}{mount}"),
                None => format!("{origin}/"),
            };
            if let Some(slug) = &options.dev_app {
                println!("privatium: {slug} at {url}");
            }
            if options.open {
                node::open_browser(&url);
            }

            let maintenance = tokio::spawn(maintain_daily(
                Arc::clone(&handler),
                slugs,
                global.verbose || options.dev,
            ));

            let served = tokio::select! {
                result = adapter::serve(listener, Arc::clone(&handler)) => result.map_err(anyhow::Error::from),
                signal = tokio::signal::ctrl_c() => {
                    signal.context("waiting for Ctrl-C")?;
                    eprintln!("privatium: stopping");
                    Ok(())
                }
            };
            maintenance.abort();
            flush(&handler);
            served
        })?;
    Ok(0)
}

/// `--port` and `--solo` hold for this run and are never written back (`§2`).
fn apply_overrides(node: &mut Node, options: &Options) -> Result<()> {
    let config = node.config_mut();
    if let Some(port) = options.port {
        config.node.port = port;
    }
    if let Some(slug) = &options.solo {
        if !privatium_core::app::manifest::is_valid_slug(slug) {
            bail!("--solo {slug:?}: not a slug (spec/protocol.md §1.1)");
        }
        config.node.mode = Mode::Solo;
        config.node.app = Some(slug.clone());
    }
    Ok(())
}

/// Write `local/state.jsonl` on the way out. A cache, so a missed flush costs a little
/// work at the next start and nothing else.
fn flush(handler: &Arc<Handler>) {
    let mut node = handler
        .node()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Err(error) = node.flush() {
        eprintln!("privatium: could not write local/state.jsonl: {error}");
    }
}

/// The scheduled maintenance of `spec/protocol.md §5`: a snapshot when one is due under
/// the policy, then retention — for `_sys` and every loaded app, now and once a day. On
/// a blocking thread, and under the node lock only to decide and to record: the log is
/// read while the lock is held, which keeps it still, and the files are written with it
/// released, so a request never waits on a checksum.
async fn maintain_daily(handler: Arc<Handler>, slugs: Vec<String>, verbose: bool) {
    let mut ticker = tokio::time::interval(MAINTENANCE_EVERY);
    loop {
        ticker.tick().await;
        let handler = Arc::clone(&handler);
        let slugs = slugs.clone();
        let pass = tokio::task::spawn_blocking(move || maintain_once(&handler, &slugs, verbose));
        if let Err(error) = pass.await {
            eprintln!("privatium: the maintenance pass panicked: {error}");
        }
    }
}

fn maintain_once(handler: &Arc<Handler>, slugs: &[String], verbose: bool) {
    let lock = || {
        handler
            .node()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    };
    let now = jiff::Timestamp::now();
    for slug in std::iter::once(privatium_core::sys::SLUG).chain(slugs.iter().map(String::as_str)) {
        // Decide and read under the lock; write with it released; record under it again.
        let job = lock().snapshot_due(slug, now);
        match job {
            Ok(Some(job)) => match job.write() {
                Ok(snapshot) => match lock().record_snapshot(&snapshot) {
                    Ok(()) => eprintln!(
                        "privatium: {slug}: snapshot {} written ({} bytes)",
                        snapshot.id, snapshot.bytes
                    ),
                    Err(error) => {
                        eprintln!(
                            "privatium: {slug}: snapshot {} not recorded: {error}",
                            snapshot.id
                        );
                    }
                },
                Err(error) => eprintln!("privatium: {slug}: snapshot not written: {error}"),
            },
            Ok(None) => {
                if verbose {
                    eprintln!("privatium: {slug}: no snapshot due");
                }
            }
            Err(error) => eprintln!("privatium: {slug}: maintenance failed: {error}"),
        }

        let retention = {
            let node = lock();
            node.snapshot_retention()
                .map(|retention| (retention, node.paths().app_snap_dir(slug)))
        };
        match retention {
            Ok((retention, dir)) => match snapshot::prune(&dir, now, &retention) {
                Ok(pruned) => {
                    for id in &pruned.removed {
                        eprintln!(
                            "privatium: {slug}: snapshot {id} pruned (spec/protocol.md §5.4)"
                        );
                    }
                    if let Err(error) = lock().record_pruned(slug, &pruned, &retention) {
                        eprintln!("privatium: {slug}: pruning not recorded: {error}");
                    }
                }
                Err(error) => eprintln!("privatium: {slug}: pruning failed: {error}"),
            },
            Err(error) => eprintln!("privatium: {slug}: maintenance failed: {error}"),
        }
    }
    if let Err(error) = lock().flush() {
        eprintln!("privatium: could not write local/state.jsonl: {error}");
    }
}
