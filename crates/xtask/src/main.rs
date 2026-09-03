// Project:  Privatium™  |  File: crates/xtask/src/main.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-08-31  |  Modified: 2026-09-03
// Summary:  Command dispatch for the repository's own checks. Deliberately not clap:
//           spec/cli.md governs the flags of `privatium`, and nothing here should ever
//           be mistaken for part of that surface.

use std::process::{Command, ExitCode};

use anyhow::{Context, Result, bail};

mod header;
mod icons;
mod repo;
mod spec_drift;

const USAGE: &str = "\
cargo xtask <command>

  header-check          every source file carries the standard header block
                        (AGENTS.md, Style)
  spec-drift [--update] warn when spec/ has changed since skills/ was last reconciled
                        (docs/skills.md §7); --update records the current contents
  icons-verify          every icon name the shell, the apps, the skills and docs/icons.md
                        refer to exists in the vendored Bootstrap Icons set (docs/icons.md)
";

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(err) => {
            eprintln!("xtask: {err:#}");
            ExitCode::FAILURE
        }
    }
}

/// `Ok(true)` when the check passed, `Ok(false)` when it found something. An `Err` means
/// the check could not run at all, which is a different failure and reads differently.
fn run() -> Result<bool> {
    let mut args = std::env::args().skip(1);
    let command = args.next();
    let rest: Vec<String> = args.collect();

    match command.as_deref() {
        Some("header-check") => header::check(&repo::root()?),
        Some("icons-verify") => icons::check(&repo::root()?),
        Some("spec-drift") => {
            let update = rest.iter().any(|a| a == "--update");
            spec_drift::check(&repo::root()?, update)
        }
        Some("--help" | "-h") | None => {
            print!("{USAGE}");
            Ok(true)
        }
        Some(other) => bail!("unknown command {other:?}\n\n{USAGE}"),
    }
}

/// Run `git` in the repository and return its stdout, trimmed.
fn git(args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .output()
        .context("could not run `git` — the repository checks need it on PATH")?;

    if !output.status.success() {
        bail!(
            "`git {}` failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    Ok(String::from_utf8(output.stdout)
        .context("git printed something that was not UTF-8")?
        .trim_end()
        .to_owned())
}
