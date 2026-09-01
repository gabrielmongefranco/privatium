// Project:  Privatium™  |  File: crates/xtask/src/repo.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-08-31  |  Modified: 2026-08-31
// Summary:  Locating the repository, listing its files, and reading them in a way that
//           gives the same answer on every platform.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// The repository root, from git rather than from a relative path, so an xtask invoked
/// from a subdirectory behaves the same as one invoked from the top.
pub fn root() -> Result<PathBuf> {
    Ok(PathBuf::from(crate::git(&[
        "rev-parse",
        "--show-toplevel",
    ])?))
}

/// Every file git knows about, plus files that exist but are not committed yet and are
/// not ignored. The second half matters: a check that only saw committed files would let
/// a freshly written file through until the moment it stopped being new.
pub fn files(root: &Path) -> Result<Vec<String>> {
    let listing = crate::git(&[
        "-C",
        &root.to_string_lossy(),
        "ls-files",
        "--cached",
        "--others",
        "--exclude-standard",
    ])?;

    let mut files: Vec<String> = listing
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect();

    files.sort();
    files.dedup();
    Ok(files)
}

/// Read a file with line endings normalized and any byte-order mark removed.
///
/// `core.autocrlf` is on in this repository, so the same file is CRLF in a Windows working
/// tree and LF in a Linux one. Without this, `spec-drift` would report every spec file as
/// changed the moment the platform changed, which is the fastest way to teach everyone to
/// ignore it.
pub fn read_normalized(path: &Path) -> Result<String> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("could not read {}", path.display()))?;
    Ok(normalize(&raw))
}

/// CRLF to LF, and no leading byte-order mark.
fn normalize(text: &str) -> String {
    text.strip_prefix('\u{feff}')
        .unwrap_or(text)
        .replace("\r\n", "\n")
}
