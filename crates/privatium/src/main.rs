// Project:  Privatium™  |  File: crates/privatium/src/main.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-08-31  |  Modified: 2026-08-31
// Summary:  Entry point. M0 has no node to start and no CLI to parse — spec/cli.md is
//           implemented in M11 and nothing here anticipates it. What this does do is
//           reference the linked engines, so the release binary CI measures really
//           contains DuckDB and Lua (docs/plans/phase-1.md §8, R1).

use anyhow::Result;

fn main() -> Result<()> {
    let engines = privatium_core::linked_engines()?;

    println!("duckdb {}", engines.duckdb);
    println!("{}", engines.lua);

    // Not a `--version` string. spec/cli.md §1 specifies that output, and the qualified
    // protocol identifier it has to carry, and M11 is where that is implemented.
    eprintln!("privatium: no node in this build — see docs/plans/phase-1.md, M0.");

    Ok(())
}
