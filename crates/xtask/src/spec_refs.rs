// Project:  Privatium™  |  File: crates/xtask/src/spec_refs.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-05  |  Modified: 2026-09-05
// Summary:  `cargo xtask lint-spec-refs`. spec/cli.md §5.2: every finding MUST carry a
//           resolvable `spec` reference, and a rule that cannot cite the document it
//           enforces does not belong in the linter. This opens every rule's reference
//           against the checkout — the document, and the numbered heading — and fails
//           naming the rule whose citation points at nothing.

use std::path::Path;

use anyhow::Result;
use privatium_core::lint::{RULES, spec_ref};

/// Resolve every rule's `spec` against `root`. `Ok(false)` names what did not resolve.
pub fn check(root: &Path) -> Result<bool> {
    let failures = spec_ref::check_rules(root);
    if failures.is_empty() {
        println!(
            "lint-spec-refs: {} rules, every reference resolves to a document and a section",
            RULES.len()
        );
        return Ok(true);
    }
    for failure in &failures {
        eprintln!("lint-spec-refs: {failure}");
    }
    eprintln!(
        "\nlint-spec-refs: {} of {} rules cite a section this checkout does not have \
         (spec/cli.md §5.2).",
        failures.len(),
        RULES.len()
    );
    Ok(false)
}
