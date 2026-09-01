// Project:  Privatium™  |  File: crates/privatium-core/tests/build_gates.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-08-31  |  Modified: 2026-08-31
// Summary:  M0's only tests. Each one retires a build risk from docs/plans/phase-1.md §8
//           on all three CI platforms, before the milestone that depends on it exists.
//           They are named for the risk they close, not for a spec section, because none
//           of them enforces a normative MUST — the spec-named tests start in M1.

// AGENTS.md, Style: unwrap() is permitted in tests, and a test that hides a failure
// behind `?` is worse than one that panics with a line number.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use ed25519_dalek::{Signer, SigningKey, Verifier};
use sha2::{Digest, Sha256};

/// R1 — the bundled DuckDB C++ build compiles, links, and executes on this platform.
#[test]
fn test_r1_duckdb_bundled_links() {
    let engines = privatium_core::linked_engines().unwrap();
    assert!(
        !engines.duckdb.is_empty(),
        "DuckDB linked but returned an empty version string"
    );
}

/// R1 — the reason DuckDB was chosen over SQLite is native `DATE` and `DECIMAL`
/// (`docs/decisions/0001 §3`). If a future size-driven build ever trims extensions, this
/// is the property that must survive; SQLite is never the answer.
#[test]
fn test_r1_duckdb_has_native_date_and_decimal() {
    let conn = duckdb::Connection::open_in_memory().unwrap();

    let date: String = conn
        .query_row("SELECT CAST(DATE '2026-08-31' AS VARCHAR)", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(date, "2026-08-31");

    // Exact, not binary floating point. 0.1 + 0.2 must be 0.30 here, which is the whole
    // argument in ADR 0001 §3. DECIMAL crosses the Lua and JavaScript boundaries as a
    // string (spec/data-api.md), so a string is what this reads back.
    let sum: String = conn
        .query_row(
            "SELECT CAST(CAST(0.1 AS DECIMAL(10,2)) + CAST(0.2 AS DECIMAL(10,2)) AS VARCHAR)",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(sum, "0.30");
}

/// R2 — the vendored Lua C build compiles and links, and it is 5.4.
///
/// Not LuaJIT (iOS forbids JIT) and not Luau (a dialect fragments the documentation and
/// the assistance that `skills/` depends on). `AGENTS.md`, Language and stack.
#[test]
fn test_r2_mlua_vendored_links_and_is_lua_54() {
    let engines = privatium_core::linked_engines().unwrap();
    assert_eq!(engines.lua, "Lua 5.4");
}

/// M1's dependency skew, caught early: `ed25519-dalek`, `rand`, and `sha2` must agree on
/// one `rand_core` and one `digest`, or node identity (`spec/protocol.md §2.1`) cannot be
/// written at all. Generate, sign, verify, and hash — the exact four operations M1 needs.
#[test]
fn test_m1_ed25519_and_sha256_agree_on_rand_core_and_digest() {
    let mut rng = rand::rng();
    let signing = SigningKey::generate(&mut rng);
    let verifying = signing.verifying_key();

    let message = b"privatium m0 build gate";
    let signature = signing.sign(message);
    assert!(verifying.verify(message, &signature).is_ok());

    // §2.1 derives the node ID from SHA-256 of the public key. This is not that derivation
    // — that is M1's, with its own spec-named test — only proof that the digest crate and
    // the signature crate can be handed the same bytes in one build.
    let digest = Sha256::digest(verifying.as_bytes());
    assert_eq!(digest.len(), 32);
}
