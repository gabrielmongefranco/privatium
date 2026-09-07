// Project:  Privatium™  |  File: crates/privatium/src/run.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-04  |  Modified: 2026-09-06
// Summary:  Bare `privatium` (spec/cli.md §2) and `privatium dev` (§3): write the example
//           apps on a first run, open the node, apply the run's overrides, load every app,
//           bind the LAN, start discovery on the bound port, and serve `core::handle` until
//           Ctrl-C. `dev` is the same node with the app named — the reloading is the host's
//           own, a stat on the next request (§3, spec/lua-api.md §7), so there is nothing
//           for this file to watch. The weekly snapshots of spec/protocol.md §5 are written
//           by a daily pass here, since a node with no request loop of its own had nobody
//           to write them. See main README.md for full license information.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use privatium::adapter;
use privatium_core::store::snapshot;
use privatium_core::{Handler, Mode, Node};

use crate::cli::Global;
use crate::{new, node};

/// What `run` and `dev` pass in.
pub struct Options {
    /// `--port`: for this run, never written to `config.toml`.
    pub port: Option<u16>,
    /// `--solo <slug>`: overrides `[node] mode` for this run (`§2`).
    pub solo: Option<String>,
    /// `--no-discovery`: start neither mDNS nor the UDP responder (`§2`).
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
    write_examples_on_first_run(global)?;
    let mut node = node::open(global)?;
    apply_overrides(&mut node, &options)?;
    let report = node.load_apps(&node::roots(&node))?;
    node::print_report(&report, global.verbose || options.dev);

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
                format!("binding IPv4 port {port} — is another node running on that port?")
            })?;
            let addr = listener.local_addr()?;
            print!("{}", adapter::announce(addr));
            // Where the data is and which rule of spec/cli.md §1 put it there, every
            // start: the platform directory is hidden on Windows, and a portable folder
            // beside the program is easy to forget.
            {
                let node = handler
                    .node()
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                eprintln!(
                    "privatium: data in {} ({})",
                    node.paths().root().display(),
                    node.paths().source().describe()
                );
                // An expired or revoked node serves its owner alone (spec/protocol.md
                // §2.3.1, §2.3.4); said here as well as on the settings page.
                match node.standing(jiff::Timestamp::now()) {
                    Ok(privatium_core::Standing::Expired) => eprintln!(
                        "privatium: this space's certificate has expired; it serves this computer \
                         alone until it is re-admitted with `privatium pair --join` \
                         (spec/protocol.md §2.3.1)"
                    ),
                    Ok(privatium_core::Standing::Revoked) => eprintln!(
                        "privatium: this space was revoked from its cluster; it serves this computer \
                         alone, and a fresh identity is needed to join again (spec/protocol.md §2.3.4)"
                    ),
                    _ => {}
                }
            }
            if global.verbose {
                match adapter::other_urls(addr.port()) {
                    Ok(urls) => for url in urls { eprintln!("privatium: interface at {url}"); },
                    Err(_) => eprintln!("privatium: could not enumerate other interfaces; use the announced address"),
                }
            }
            // The port is settled only now (`--port 0` asks the OS), and it is what the
            // pairing URL and the TXT record's `p` key carry; discovery starts on it.
            start_discovery(&handler, addr.port(), options.no_discovery, global.verbose)?;
            let origin = format!("http://127.0.0.1:{}", addr.port());
            let url = match &dev_mount {
                Some(mount) => format!("{origin}{mount}"),
                None => format!("{origin}/"),
            };
            if let Some(slug) = &options.dev_app {
                println!("privatium: {slug} at {url}");
            }
            if options.open {
                if !options.dev {
                    print_qr_and_first_run_code(&handler)?;
                }
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

/// The first run of `§2`: an `apps/` that holds no app folder gets the example apps
/// written into it before the node opens, so the launcher is never empty — on a data
/// directory that is brand new and on one that was used before the binary carried the
/// examples alike. A checkout already mounts the repository's copies as `bundled`
/// (`node::checkout_apps`) and gets nothing written, since a second copy of each slug
/// would shadow the one the developer is editing.
fn write_examples_on_first_run(global: &Global) -> Result<()> {
    let paths = node::paths(global)?;
    if !node::apps_dir_is_empty(&paths) {
        return Ok(());
    }
    if node::checkout_apps().is_some() {
        if global.verbose {
            eprintln!(
                "privatium: running from a checkout — the repository's apps/ serves the \
                 example apps, so none are written to {}",
                paths.apps_dir().display()
            );
        }
        return Ok(());
    }
    let apps_dir = paths.apps_dir();
    let written = new::write_examples(&apps_dir)
        .with_context(|| format!("writing the example apps to {}", apps_dir.display()))?;
    eprintln!(
        "privatium: no apps yet — {} example app(s) written to {} ({} files); edit or \
         delete them freely (spec/cli.md §2)",
        privatium_core::app::examples::SLUGS.len(),
        apps_dir.display(),
        written.len()
    );
    Ok(())
}

/// Settle the bound port into this run's configuration, then start discovery
/// (`spec/protocol.md §6`, `spec/cli.md §2`) unless `--no-discovery` was given. Each
/// mechanism reports its own outcome: a refusal by the platform is a line here and never
/// a reason for the node not to serve.
fn start_discovery(
    handler: &Arc<Handler>,
    port: u16,
    no_discovery: bool,
    verbose: bool,
) -> Result<()> {
    let mut node = handler
        .node()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    node.config_mut().node.port = port;
    if no_discovery {
        eprintln!(
            "privatium: --no-discovery: not advertising on this network; other devices \
             reach this node by its URL alone (spec/protocol.md §6)"
        );
        return Ok(());
    }
    node.serve_discovery()
        .context("starting discovery (spec/protocol.md §6)")?;
    if let Some(status) = node.discovery_status() {
        for (name, outcome) in [("mDNS", &status.mdns), ("UDP 52525", &status.udp)] {
            match outcome {
                privatium_core::discover::Outcome::Started if verbose => {
                    eprintln!("privatium: discovery: {name} started");
                }
                privatium_core::discover::Outcome::Started => {}
                privatium_core::discover::Outcome::Off => {
                    eprintln!("privatium: discovery: {name} is off in sys_setting");
                }
                privatium_core::discover::Outcome::Failed(why) => {
                    eprintln!("privatium: discovery: {name} not started: {why}");
                }
            }
        }
        if !status.any_started() {
            eprintln!(
                "privatium: discovery: nothing is advertising; other devices reach this \
                 node by its URL alone"
            );
        }
    }
    Ok(())
}

/// `--open` on a bare run (`spec/cli.md §2`): the QR code of the LAN URL with the URL in
/// text beside it and, on a node whose `sys_device` holds no row but its own, one
/// pairing window opened as the node starts with its code printed beneath the QR code
/// (`spec/protocol.md §7.1`, first run). Once any device has paired, only the QR code.
fn print_qr_and_first_run_code(handler: &Arc<Handler>) -> Result<()> {
    let mut node = handler
        .node()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let url = node.listen_url();
    println!("privatium: on a phone on this network, scan this code or open {url}");
    match privatium_core::pair::qr::text(&url) {
        Some(code) => print!("{code}"),
        None => println!("privatium: (the URL is too long for a QR code; type it)"),
    }
    if node.has_paired_device()? {
        return Ok(());
    }
    let window = node
        .pair(privatium_core::pair::TTL)
        .context("opening the first-run pairing window (spec/protocol.md §7.1)")?;
    println!(
        "privatium: no device has paired yet, so pairing is open for {} seconds — on the \
         phone, tap these four emoji in order:",
        privatium_core::pair::TTL.as_secs()
    );
    for (glyph, label) in window.emoji.iter().zip(window.labels.iter()) {
        println!("    {glyph}  {label}");
    }
    println!(
        "privatium: or type these two words: {} {}",
        window.words[0], window.words[1]
    );
    Ok(())
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
/// a blocking thread, and under the node lock only to decide and to record: the job takes
/// each log segment's length under the lock, then reads that bounded prefix and writes the
/// files with it released, so a request never waits on a read or a checksum.
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
        // Decide under the lock; read and write with it released; record under it again.
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
