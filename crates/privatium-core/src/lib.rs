// Project:  Privatium™  |  File: crates/privatium-core/src/lib.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-08-31  |  Modified: 2026-08-31
// Summary:  Crate root. M0 is scaffolding: the only thing here is the linkage probe that
//           risk R1 asks for. The log, store, app loader, Lua host, wire interface, HTTP
//           layer, and linter arrive as their own modules in M2 onward.

//! Privatium core.
//!
//! The contract this crate implements is `spec/protocol.md` and `spec/app-contract.md`.
//! Neither is optional reading, and where this code and those documents disagree, they
//! are right and this is a bug.
//!
//! At M0 the crate has no behaviour. What it does have is a compiled, linked DuckDB and
//! a compiled, linked Lua, because both are C/C++ builds whose cross-platform behaviour
//! is a risk the plan wants retired before M3 and M7 depend on them
//! (`docs/plans/phase-1.md §8`, R1 and R2).

use thiserror::Error;

/// Versions of the two foreign engines statically linked into this build.
///
/// Reported by `privatium` so the number CI prints for binary size is the size of a
/// binary that genuinely contains both engines, rather than one where the linker
/// discarded them as unreferenced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedEngines {
    /// DuckDB's own `version()`, e.g. `v1.5.1`.
    pub duckdb: String,
    /// Lua's `_VERSION`, which must be `Lua 5.4` — not LuaJIT, not Luau (`AGENTS.md`).
    pub lua: String,
}

/// Failures of the linkage probe.
#[derive(Debug, Error)]
pub enum EngineError {
    /// DuckDB linked but did not answer.
    #[error("bundled DuckDB failed to answer version(): {0}")]
    DuckDb(#[from] duckdb::Error),
    /// Lua linked but did not answer.
    #[error("vendored Lua failed to answer _VERSION: {0}")]
    Lua(#[from] mlua::Error),
}

/// Open an in-memory DuckDB and a fresh Lua state, and ask each its version.
///
/// This exists to fail loudly at M0 on any platform where the bundled C++ or vendored C
/// build is broken, rather than at M3 or M7 when there is real code to blame it on.
pub fn linked_engines() -> Result<LinkedEngines, EngineError> {
    let conn = duckdb::Connection::open_in_memory()?;
    let duckdb: String = conn.query_row("SELECT version()", [], |row| row.get(0))?;

    let lua = mlua::Lua::new();
    let lua: String = lua.load("return _VERSION").eval()?;

    Ok(LinkedEngines { duckdb, lua })
}
