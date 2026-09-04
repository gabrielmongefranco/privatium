// Project:  Privatium™  |  File: crates/privatium/src/main.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-08-31  |  Modified: 2026-09-04
// Summary:  Entry point: spec/cli.md. Bare `privatium` runs a node; `dev`, `new`, `skill`,
//           `snapshot` and `restore` are the subcommands of Phase 1; `lint` is M12's and
//           `pair` and `firewall` are later phases', and all three parse and say so rather
//           than being absent, so the help text matches the spec. Exit codes are §1's.
//           Errors are anyhow at this boundary (AGENTS.md, Style) and print as one line.

use std::process::ExitCode;

mod cli;
mod data;
mod new;
mod node;
mod run;
mod skill;

use cli::Command;

/// The protocol claim of `spec/cli.md §1`: Phase 1 does not satisfy every item of
/// `spec/protocol.md §13`, so the string is qualified rather than a bare `pv/1`
/// (`docs/plans/phase-1.md §2.1`). The wire format itself is `privatium_core::PROTOCOL`.
fn protocol_claim() -> String {
    format!("{} (partial: phase 1)", privatium_core::PROTOCOL)
}

/// `--version`: the build version and the protocol it implements, on one line.
fn version_line() -> String {
    format!(
        "privatium {} {}",
        env!("CARGO_PKG_VERSION"),
        protocol_claim()
    )
}

fn main() -> ExitCode {
    let invocation = match cli::parse(std::env::args_os().skip(1)) {
        Ok(invocation) => invocation,
        Err(usage) => {
            eprintln!("privatium: {usage}\n\n{}", cli::HELP);
            return ExitCode::from(2);
        }
    };

    let outcome = match invocation.command {
        Command::Version => {
            println!("{}", version_line());
            Ok(0)
        }
        Command::Help => {
            print!("{}", cli::HELP);
            Ok(0)
        }
        Command::Run {
            port,
            solo,
            no_discovery,
            open,
        } => run::run(
            &invocation.global,
            run::Options {
                port,
                solo,
                no_discovery,
                open,
                dev_app: None,
                dev: false,
            },
        ),
        Command::Dev { app, open } => run::run(
            &invocation.global,
            run::Options {
                port: None,
                solo: None,
                no_discovery: false,
                open,
                dev_app: app,
                dev: true,
            },
        ),
        Command::New {
            slug,
            tier,
            from,
            scaffold,
        } => new::new(
            &invocation.global,
            &slug,
            tier,
            from.as_deref(),
            scaffold.as_deref(),
        ),
        Command::Lint { .. } => not_in_this_build(
            "lint",
            "the linter is M12 of docs/plans/phase-1.md; spec/cli.md §5 is its contract",
        ),
        Command::SkillList => skill::list(),
        Command::SkillExport { names, out } => skill::export(&names, out.as_deref()),
        Command::Snapshot { app, verify } => {
            data::snapshot(&invocation.global, app.as_deref(), verify)
        }
        Command::Restore { from, app, dry_run } => {
            data::restore(&invocation.global, &from, app.as_deref(), dry_run)
        }
        Command::Pair { .. } => not_in_this_build(
            "pair",
            "pairing is Phase 2 of docs/roadmap.md; spec/cli.md §8 and spec/protocol.md §7 are its contract",
        ),
        Command::Firewall { .. } => not_in_this_build(
            "firewall",
            "the firewall helper is Phase 6 of docs/roadmap.md; spec/cli.md §9 is its contract",
        ),
    };

    match outcome {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("privatium: {error:#}");
            ExitCode::from(1)
        }
    }
}

/// A command the spec has and this build does not (`docs/plans/phase-1.md`, M11): it
/// parses, so the help text is the spec's, and it says exactly why it stops.
fn not_in_this_build(command: &str, why: &str) -> anyhow::Result<u8> {
    eprintln!("privatium {command}: not in this build — {why}");
    Ok(1)
}
