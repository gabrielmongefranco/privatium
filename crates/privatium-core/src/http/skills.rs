// This file is part of Privatium
// crates/privatium-core/src/http/skills.rs
// Author(s): Gabriel Mongefranco
// Created: 2026-09-03
// Last Modified: 2026-10-04
// Summary: /skills/<name>.md and /skills/bundle.zip (spec/cli.md §6, docs/skills.md §6): the app-skills/
//          tree of this build, embedded so an owner gets the contract matching the version
//          they are running. The bundle is a stored (uncompressed) zip written by hand — a
//          hundred kilobytes of Markdown does not justify a compression crate.
// Notes: See README file for documentation and full license information.
//
// Copyright © 2026 Gabriel Mongefranco
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License along
// with this program. If not, see <https://www.gnu.org/licenses/>.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use include_dir::{Dir, include_dir};
use sha2::{Digest as _, Sha256};

/// `app-skills/` at the repository root, as of this build: the skills shipped to app
/// authors, kept apart from `skills/`, which holds the skills for working on this
/// repository and ships nowhere.
static SKILLS: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../../app-skills");

/// The file each skill folder is served as.
const SKILL_FILE: &str = "SKILL.md";

/// Every skill folder name, sorted.
#[must_use]
pub fn names() -> Vec<String> {
    let mut names: Vec<String> = SKILLS
        .dirs()
        .filter(|dir| dir.get_file(dir.path().join(SKILL_FILE)).is_some())
        .filter_map(|dir| dir.path().file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// `app-skills/<name>/SKILL.md`, if `name` is a skill.
#[must_use]
pub fn skill(name: &str) -> Option<&'static str> {
    if name.contains(['/', '\\', '.']) {
        return None;
    }
    SKILLS
        .get_file(format!("{name}/{SKILL_FILE}"))
        .and_then(|file| file.contents_utf8())
}

/// The strong `ETag` of `/skills/<name>.md` (`spec/protocol.md §9.3`): the document's
/// SHA-256 in quotes, computed once per process. `None` when `name` is not a skill.
#[must_use]
pub fn etag(name: &str) -> Option<&'static str> {
    static ETAGS: LazyLock<BTreeMap<String, String>> = LazyLock::new(|| {
        names()
            .into_iter()
            .filter_map(|name| skill(&name).map(|text| (name, quoted_hash(text.as_bytes()))))
            .collect()
    });
    ETAGS.get(name).map(String::as_str)
}

/// The strong `ETag` of `/skills/bundle.zip`, computed once per process over the bundle.
#[must_use]
pub fn bundle_etag() -> &'static str {
    static ETAG: LazyLock<String> = LazyLock::new(|| quoted_hash(bundle()));
    ETAG.as_str()
}

fn quoted_hash(bytes: &[u8]) -> String {
    format!("\"sha256-{}\"", STANDARD.encode(Sha256::digest(bytes)))
}

/// Every file under `app-skills/` at its path relative to that folder, slash-separated, sorted —
/// what `privatium skill export` writes to disk (`spec/cli.md §6`) and what the bundle
/// below holds.
#[must_use]
pub fn files() -> Vec<(String, &'static [u8])> {
    let mut entries: Vec<(String, &[u8])> = Vec::new();
    collect(&SKILLS, &mut entries);
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries
}

/// `/skills/bundle.zip`: every file under `app-skills/` — `README.md`, each skill's `SKILL.md`
/// and its `reference/` — at its path relative to that folder, so extracting the archive
/// into a `skills/` folder reproduces the tree `privatium skill export` writes
/// (`spec/cli.md §6`).
///
/// Built once per process; the same bytes every time, since the entries carry a fixed
/// timestamp rather than the moment of the request.
#[must_use]
pub fn bundle() -> &'static [u8] {
    static BUNDLE: LazyLock<Vec<u8>> = LazyLock::new(|| {
        let f = files();
        let refs: Vec<(String, &[u8])> = f.into_iter().map(|(k, v)| (k, v as &[u8])).collect();
        crate::zip::stored(&refs)
    });
    BUNDLE.as_slice()
}

fn collect<'a>(dir: &'a Dir<'a>, into: &mut Vec<(String, &'a [u8])>) {
    for file in dir.files() {
        let name = file.path().to_string_lossy().replace('\\', "/");
        into.push((name, file.contents()));
    }
    for sub in dir.dirs() {
        collect(sub, into);
    }
}

// AGENTS.md, Style: unwrap() is permitted in tests. The crate-level deny reaches unit
// tests inside src/, so each one opts out where it is declared.
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_seven_skills_are_embedded() {
        let names = names();
        assert_eq!(
            names,
            [
                "privatium-accessibility",
                "privatium-games",
                "privatium-overview",
                "privatium-security",
                "privatium-tier1-lua",
                "privatium-tier2-web",
                "privatium-tier3-rust",
            ]
        );
        assert!(
            skill("privatium-overview")
                .unwrap()
                .contains("privatium lint")
        );
        assert!(skill("../README").is_none());
        assert!(skill("nope").is_none());
    }

    /// The check value from the CRC-32 specification.
    #[test]
    fn crc32_check_value() {
        assert_eq!(crate::zip::crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crate::zip::crc32(b""), 0);
    }

    /// Walk the archive by hand: every local header is where the central directory says,
    /// every name is the path it was registered under, and the record count matches.
    #[test]
    fn the_bundle_is_a_well_formed_stored_zip_of_the_skills_tree() {
        let bytes = bundle();
        let eocd = bytes.len() - 22;
        assert_eq!(&bytes[eocd..eocd + 4], &0x0605_4b50u32.to_le_bytes());
        let count = u16::from_le_bytes([bytes[eocd + 10], bytes[eocd + 11]]) as usize;
        let central_size =
            u32::from_le_bytes(bytes[eocd + 12..eocd + 16].try_into().unwrap()) as usize;
        let central_offset =
            u32::from_le_bytes(bytes[eocd + 16..eocd + 20].try_into().unwrap()) as usize;
        assert_eq!(central_offset + central_size, eocd);

        let mut names = Vec::new();
        let mut at = central_offset;
        for _ in 0..count {
            assert_eq!(&bytes[at..at + 4], &0x0201_4b50u32.to_le_bytes());
            let method = u16::from_le_bytes([bytes[at + 10], bytes[at + 11]]);
            assert_eq!(method, 0, "stored only");
            let crc = u32::from_le_bytes(bytes[at + 16..at + 20].try_into().unwrap());
            let size = u32::from_le_bytes(bytes[at + 24..at + 28].try_into().unwrap()) as usize;
            let name_len = u16::from_le_bytes([bytes[at + 28], bytes[at + 29]]) as usize;
            let offset = u32::from_le_bytes(bytes[at + 42..at + 46].try_into().unwrap()) as usize;
            let name = std::str::from_utf8(&bytes[at + 46..at + 46 + name_len]).unwrap();
            // The local header it points at, and the data behind it.
            assert_eq!(&bytes[offset..offset + 4], &0x0403_4b50u32.to_le_bytes());
            let data_at = offset + 30 + name_len;
            assert_eq!(crate::zip::crc32(&bytes[data_at..data_at + size]), crc);
            names.push(name.to_owned());
            at += 46 + name_len;
        }
        assert!(names.contains(&"README.md".to_owned()), "{names:?}");
        assert!(names.contains(&"privatium-overview/SKILL.md".to_owned()));
        assert!(names.contains(&"privatium-tier1-lua/reference/README.md".to_owned()));
        assert!(
            names
                .iter()
                .all(|n| !n.starts_with('/') && !n.contains(".."))
        );
        assert_eq!(names.len(), count);
        // Deterministic.
        assert_eq!(bundle(), bytes);
    }
}
