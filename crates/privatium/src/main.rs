// Project:  Privatium™  |  File: crates/privatium/src/main.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-08-31  |  Modified: 2026-09-03
// Summary:  Entry point. Still M0's placeholder: no CLI is parsed — spec/cli.md is M11's and
//           nothing here anticipates it. What it does do is reference the linked engines and
//           the adapter, so the release binary CI measures really contains DuckDB, Lua and
//           the HTTP stack (docs/plans/phase-1.md §8, R1). A development start exists behind
//           an environment variable and is deliberately not part of any documented surface.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, Result};
use privatium::adapter;
use privatium_core::{AppRoot, Handler, Node};

/// Set this to a data directory and the binary opens a node there and serves it on
/// loopback. Temporary: it exists so the M6 adapter can be exercised end to end before the
/// CLI of M11 gives `privatium` its real start, and it will go when that lands. It is not
/// in `spec/cli.md` and must not be documented as if it were.
const DEV_SERVE: &str = "PRIVATIUM_DEV_SERVE";

fn main() -> Result<()> {
    let engines = privatium_core::linked_engines()?;

    println!("duckdb {}", engines.duckdb);
    println!("{}", engines.lua);

    if let Some(dir) = std::env::var_os(DEV_SERVE) {
        return serve_dev(PathBuf::from(dir));
    }

    // Not a `--version` string. spec/cli.md §1 specifies that output, and the qualified
    // protocol identifier it has to carry, and M11 is where that is implemented.
    eprintln!("privatium: no node in this build — see docs/plans/phase-1.md, M0.");

    Ok(())
}

/// Open the node at `dir`, load every app root, and serve until interrupted.
fn serve_dev(dir: PathBuf) -> Result<()> {
    let mut node = Node::open(&dir).with_context(|| format!("opening {}", dir.display()))?;

    // Every root in one call, or "folder missing" is wrong (M5). The repository's `apps/`
    // is bundled only when running from a checkout, which is what a development start is.
    let mut roots = vec![AppRoot::local(node.paths().apps_dir())];
    let checkout = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps");
    if checkout.is_dir() {
        roots.push(AppRoot::bundled(checkout));
    }
    let report = node.load_apps(&roots)?;
    for slug in &report.loaded {
        eprintln!("privatium: loaded {slug}");
    }
    for failure in &report.failed {
        eprintln!("privatium: not loaded — {failure}");
    }
    for warning in &report.warnings {
        eprintln!("privatium: warning — {warning}");
    }

    let port = node.config().node.port;
    let handler = Arc::new(Handler::new(node, report));

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async {
            let listener = adapter::bind(port)
                .await
                .with_context(|| format!("binding 127.0.0.1:{port}"))?;
            print!("{}", adapter::announce(listener.local_addr()?));
            adapter::serve(listener, handler).await?;
            Ok(())
        })
}
