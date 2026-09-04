// Project:  Privatium™  |  File: crates/privatium-core/src/lint/spec_ref.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-05  |  Modified: 2026-09-05
// Summary:  What makes a finding's `spec` field resolvable (spec/cli.md §5.2): a
//           reference is `<path> §<section>` or a bare `<path>`, the path is a document
//           under spec/ or docs/, and the section is a numbered heading of it. The core's
//           own test and `cargo xtask lint-spec-refs` both resolve every rule through this
//           against a checkout, so a rule cannot cite a section that is not there.

use std::path::Path;

/// A parsed reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecRef {
    /// `spec/cli.md`, repository-relative with forward slashes.
    pub path: String,
    /// `5.1`, when the reference names a section.
    pub section: Option<String>,
}

/// Parse `spec/cli.md §5.1` or `docs/icons.md`.
#[must_use]
pub fn parse(text: &str) -> Option<SpecRef> {
    let text = text.trim();
    let (path, section) = match text.split_once('§') {
        Some((path, section)) => (path.trim(), Some(section.trim().to_owned())),
        None => (text, None),
    };
    if path.is_empty()
        || !(path.starts_with("spec/") || path.starts_with("docs/"))
        || !path.ends_with(".md")
        || path.contains("..")
    {
        return None;
    }
    if section.as_deref().is_some_and(|s| {
        s.is_empty()
            || !s
                .bytes()
                .all(|b| b.is_ascii_digit() || b == b'.' || b.is_ascii_alphabetic())
    }) {
        return None;
    }
    Some(SpecRef {
        path: path.to_owned(),
        section,
    })
}

/// Whether `heading` — a Markdown heading line — is the section numbered `section`:
/// `## 5. …` for `5`, `### 5.1 …` for `5.1`, `### 3.1b …` for `3.1b`.
#[must_use]
pub fn heading_is(heading: &str, section: &str) -> bool {
    let text = heading.trim_start_matches('#').trim_start();
    let number: String = text
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.' || c.is_ascii_alphabetic())
        .collect();
    let number = number.trim_end_matches('.');
    !number.is_empty()
        && number == section
        && text[number.len()..]
            .chars()
            .next()
            .is_none_or(|c| c == '.' || c.is_whitespace())
}

/// Resolve `text` against a checkout at `root`: the document exists and, when a section
/// is named, a heading carries its number. The error says which half failed.
pub fn resolve(root: &Path, text: &str) -> Result<(), String> {
    let parsed =
        parse(text).ok_or_else(|| format!("{text:?} is not `<spec or docs path> [§section]`"))?;
    let path = root.join(&parsed.path);
    let contents =
        std::fs::read_to_string(&path).map_err(|error| format!("{}: {error}", parsed.path))?;
    let Some(section) = parsed.section else {
        return Ok(());
    };
    let found = contents
        .lines()
        .filter(|line| line.starts_with('#'))
        .any(|line| heading_is(line, &section));
    if found {
        Ok(())
    } else {
        Err(format!("{}: no heading numbered §{section}", parsed.path))
    }
}

/// Every rule's reference resolved against `root`; the failures, if any.
#[must_use]
pub fn check_rules(root: &Path) -> Vec<String> {
    super::RULES
        .iter()
        .filter_map(|rule| {
            resolve(root, rule.spec)
                .err()
                .map(|why| format!("{}: {why}", rule.id))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references_parse_and_headings_match() {
        assert_eq!(
            parse("spec/cli.md §5.1"),
            Some(SpecRef {
                path: "spec/cli.md".into(),
                section: Some("5.1".into())
            })
        );
        assert_eq!(parse("docs/icons.md").map(|r| r.section), Some(None));
        assert_eq!(parse("README.md §1"), None);
        assert_eq!(parse("spec/../x.md"), None);
        assert!(heading_is("## 5. `privatium lint`", "5"));
        assert!(heading_is("### 5.1 Rule classes", "5.1"));
        assert!(heading_is("### 3.1b `sys_cluster`", "3.1b"));
        assert!(!heading_is("### 5.1 Rule classes", "5"));
        assert!(!heading_is("### 51 x", "5"));
    }
}
