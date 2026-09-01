// Project:  Privatium™  |  File: crates/xtask/src/spec_drift.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-08-31  |  Modified: 2026-08-31
// Summary:  `cargo xtask spec-drift`. docs/skills.md §7 makes a change to spec/ that is
//           not reflected in skills/ an incomplete change. Until the generator exists in
//           M13 there is nothing to diff, so this records what spec/ looked like when
//           skills/ was last reconciled and warns when that stops being true.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// Where the recorded hashes live. Not under `skills/`, because it is not part of what a
/// node serves at `/skills/<name>.md`, and not shipped in the binary either.
const MANIFEST: &str = "crates/xtask/spec-hashes.toml";

/// M0 warns; docs/plans/phase-1.md §8 (R8) promotes this to an error in M13, when
/// `xtask gen-skill-reference` can say what actually drifted rather than only that
/// something did. Flipping this constant is the promotion.
const FAIL_ON_DRIFT: bool = false;

const HEADER: &str = "\
# Project:  Privatium™  |  File: crates/xtask/spec-hashes.toml
# Authors:  Gabriel Mongefranco (@gabrielmongefranco)
# Created:  2026-08-31  |  Modified: 2026-08-31
# Summary:  SHA-256 of every normative document as of the last time skills/ was brought
#           into line with it (docs/skills.md §7). Regenerate with
#           `cargo xtask spec-drift --update`, and only once skills/ actually agrees —
#           updating this file is the act of claiming the reconciliation happened.
#
#           Hashes are taken over the file with CRLF folded to LF and any byte-order mark
#           removed, so a Windows working tree and a Linux one agree.

";

#[derive(Debug, Default, Deserialize)]
struct Manifest {
    #[serde(default)]
    hashes: BTreeMap<String, String>,
}

/// Compare `spec/` against the recorded hashes, or record the current ones.
pub fn check(root: &Path, update: bool) -> Result<bool> {
    let current = hash_spec(root)?;

    if update {
        write_manifest(root, &current)?;
        println!(
            "spec-drift: recorded {} documents in {MANIFEST}",
            current.len()
        );
        return Ok(true);
    }

    let recorded = read_manifest(root)?;
    let mut drifted = Vec::new();

    for (path, hash) in &current {
        match recorded.hashes.get(path) {
            None => drifted.push(format!("{path} is new since skills/ was last reconciled")),
            Some(previous) if previous != hash => {
                drifted.push(format!(
                    "{path} has changed since skills/ was last reconciled"
                ));
            }
            Some(_) => {}
        }
    }
    for path in recorded.hashes.keys() {
        if !current.contains_key(path) {
            drifted.push(format!("{path} was removed but is still recorded"));
        }
    }

    if drifted.is_empty() {
        println!("spec-drift: {} documents, none changed", current.len());
        return Ok(true);
    }

    for finding in &drifted {
        eprintln!("spec-drift: {finding}");
    }
    eprintln!(
        "\nspec-drift: a change to spec/ that is not reflected in skills/ is an incomplete \
         change (docs/skills.md §7).\n\
         Bring skills/ into line, then run `cargo xtask spec-drift --update` in the same \
         commit."
    );

    Ok(!FAIL_ON_DRIFT)
}

/// SHA-256 of every markdown document under `spec/`, keyed by repository-relative path.
fn hash_spec(root: &Path) -> Result<BTreeMap<String, String>> {
    let mut hashes = BTreeMap::new();

    for path in crate::repo::files(root)? {
        if !path.starts_with("spec/") || !path.ends_with(".md") {
            continue;
        }
        let contents = crate::repo::read_normalized(&root.join(&path))?;
        let digest = Sha256::digest(contents.as_bytes());
        hashes.insert(path, hex(&digest));
    }

    Ok(hashes)
}

fn read_manifest(root: &Path) -> Result<Manifest> {
    let path = manifest_path(root);
    if !path.is_file() {
        // Not an error: the first run in a fresh clone of a branch that predates the
        // manifest should say what to do, not fall over.
        eprintln!("spec-drift: no {MANIFEST} yet — run `cargo xtask spec-drift --update`");
        return Ok(Manifest::default());
    }

    let text = crate::repo::read_normalized(&path)?;
    toml::from_str(&text).with_context(|| format!("could not parse {MANIFEST}"))
}

fn write_manifest(root: &Path, hashes: &BTreeMap<String, String>) -> Result<()> {
    let mut out = String::from(HEADER);
    out.push_str("[hashes]\n");
    for (path, hash) in hashes {
        out.push_str(&format!("\"{path}\" = \"{hash}\"\n"));
    }

    let path = manifest_path(root);
    std::fs::write(&path, out).with_context(|| format!("could not write {}", path.display()))
}

fn manifest_path(root: &Path) -> PathBuf {
    root.join(MANIFEST)
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_is_lowercase_and_padded() {
        assert_eq!(hex(&[0x00, 0x0f, 0xff]), "000fff");
    }

    #[test]
    fn a_manifest_round_trips() {
        let text = "[hashes]\n\"spec/cli.md\" = \"abc\"\n";
        let manifest: Manifest = match toml::from_str(text) {
            Ok(manifest) => manifest,
            Err(error) => panic!("{error}"),
        };
        assert_eq!(
            manifest.hashes.get("spec/cli.md").map(String::as_str),
            Some("abc")
        );
    }
}
