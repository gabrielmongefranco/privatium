// Project:  Privatium™  |  File: crates/privatium/src/node.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-04  |  Modified: 2026-09-04
// Summary:  What every subcommand that touches a node shares: opening it from the two
//           global flags of spec/cli.md §1, the app roots it loads (the owner's apps/ and,
//           in a checkout, the repository's reference apps as bundled), the load report
//           printed the same way everywhere, and the browser opener `--open` uses.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context as _, Result};
use privatium_core::{AppRoot, LoadReport, Node, Paths};

use crate::cli::Global;

/// The node the global flags name, opened — or created on first run.
pub fn open(global: &Global) -> Result<Node> {
    Node::open_with(global.data_dir.as_deref(), global.config.as_deref()).with_context(|| {
        match &global.data_dir {
            Some(dir) => format!("opening the node at {}", dir.display()),
            None => "opening the node in the platform data directory".to_owned(),
        }
    })
}

/// The paths the global flags name, without opening anything — for `new`, which writes
/// into `apps/` and needs no node, and for `restore`, which copies before opening.
pub fn paths(global: &Global) -> Result<Paths> {
    Paths::resolve(global.data_dir.as_deref(), global.config.as_deref())
        .context("resolving the data directory")
}

/// The repository's `apps/`, when this binary runs from a checkout.
///
/// A development start is the one situation in which the reference apps are on disk
/// beside the binary's source; a packaged binary (M13) carries them another way. The
/// path is fixed at compile time and simply absent anywhere else.
#[must_use]
pub fn checkout_apps() -> Option<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()?
        .parent()?
        .join("apps");
    dir.is_dir().then_some(dir)
}

/// Every root in one call, or "folder missing" is wrong (`Node::load_apps`).
#[must_use]
pub fn roots(node: &Node) -> Vec<AppRoot> {
    let mut roots = vec![AppRoot::local(node.paths().apps_dir())];
    if let Some(checkout) = checkout_apps() {
        roots.push(AppRoot::bundled(checkout));
    }
    roots
}

/// Open the node and load its apps, printing the report.
pub fn open_loaded(global: &Global) -> Result<(Node, LoadReport)> {
    let mut node = open(global)?;
    let report = node.load_apps(&roots(&node))?;
    print_report(&report, global.verbose);
    Ok((node, report))
}

/// What `load_apps` found, to standard error: failures and warnings always, the loaded
/// list under `--verbose`.
pub fn print_report(report: &LoadReport, verbose: bool) {
    if verbose {
        for slug in &report.loaded {
            eprintln!("privatium: loaded {slug}");
        }
        for slug in &report.disabled {
            eprintln!("privatium: {slug} is disabled");
        }
        for slug in &report.missing {
            eprintln!("privatium: {slug}: folder missing");
        }
    }
    for failure in &report.failed {
        eprintln!("privatium: not loaded — {failure}");
    }
    for warning in &report.warnings {
        eprintln!("privatium: warning — {warning}");
    }
}

/// Why `slug` did not load, from the report, for a message.
#[must_use]
pub fn failure_of(report: &LoadReport, slug: &str) -> Option<String> {
    report
        .failed
        .iter()
        .find(|failure| failure.folder == slug)
        .map(ToString::to_string)
}

/// Open `url` in the owner's browser (`spec/cli.md §2`, `--open`), through the platform's
/// opener. Best effort: a failure is a line on standard error, never a reason to stop
/// the node.
pub fn open_browser(url: &str) {
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = Command::new("cmd");
        command.args(["/C", "start", "", url]);
        command
    };
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = Command::new("open");
        command.arg(url);
        command
    };
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let mut command = {
        let mut command = Command::new("xdg-open");
        command.arg(url);
        command
    };
    let result = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    match result {
        Ok(_) => eprintln!("privatium: opening {url}"),
        Err(error) => eprintln!("privatium: could not open a browser: {error}"),
    }
}
