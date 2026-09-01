// Project:  Privatium™  |  File: crates/xtask/src/header.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-08-31  |  Modified: 2026-08-31
// Summary:  `cargo xtask header-check`. AGENTS.md requires a header block on every source
//           file; this is what turns that from a habit into a gate.

use std::path::Path;

use anyhow::Result;

/// How many lines from the top of a file the header block may occupy.
const HEADER_WINDOW: usize = 25;

/// Extensions that must carry a header, and the comment openers each accepts.
///
/// `.md` is here but is further narrowed to `spec/` and `docs/` by [`in_scope`]: the
/// root-level documents, every `apps/**` README and SKILL.md, and everything under
/// `skills/` are exempt by design — they are prose for humans and assistants, and a
/// provenance block at the top of `README.md` would be noise.
///
/// `.html` and `.toml` are deliberately absent. Manifests in this repository carry a
/// header by convention and should keep doing so, but the convention is not enforced.
const CHECKED: &[(&str, &[&str])] = &[
    ("rs", &["//", "/*"]),
    ("lua", &["--"]),
    ("sql", &["--"]),
    ("js", &["//", "/*"]),
    ("css", &["//", "/*"]),
    ("lsp", &["<?--"]),
    ("md", &["<!--"]),
];

/// What a header must contain. `.lsp` files carry a reduced form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// `Project:`, the file's own path, `Authors:`, `Created:`, `Modified:`, `Summary:`.
    Full,
    /// `Project:`, the file's own path, `Summary:`. Templates are read alongside the app
    /// they belong to, and repeating its authorship on every partial helps nobody.
    Template,
}

/// Check every in-scope file. Returns `false` when at least one is missing a header.
pub fn check(root: &Path) -> Result<bool> {
    let mut checked = 0usize;
    let mut findings = Vec::new();

    for path in crate::repo::files(root)? {
        let Some(shape) = in_scope(root, &path) else {
            continue;
        };
        checked += 1;

        let contents = crate::repo::read_normalized(&root.join(&path))?;
        for problem in problems(&path, &contents, shape) {
            findings.push(format!("{path}: {problem}"));
        }
    }

    if findings.is_empty() {
        println!("header-check: {checked} files, all carry a header block");
        return Ok(true);
    }

    for finding in &findings {
        eprintln!("header-check: {finding}");
    }
    eprintln!(
        "\nheader-check: {} problems across {checked} files. \
         The format is in AGENTS.md, under Style.",
        findings.len()
    );
    Ok(false)
}

/// The shape required of `path`, or `None` if the file is not checked.
fn in_scope(root: &Path, path: &str) -> Option<Shape> {
    let extension = path.rsplit_once('.').map(|(_, ext)| ext)?;
    if !CHECKED.iter().any(|(checked, _)| *checked == extension) {
        return None;
    }

    if extension == "md" && !(path.starts_with("spec/") || path.starts_with("docs/")) {
        return None;
    }
    if is_vendored(root, path) {
        return None;
    }

    Some(if extension == "lsp" {
        Shape::Template
    } else {
        Shape::Full
    })
}

/// Third-party code carries its own provenance and must not be given ours.
///
/// The marker is a `VENDOR.md` beside the file or above it, which this repository already
/// requires of anything vendored (`apps/animals/static/VENDOR.md` is the worked example).
/// Minified bundles are excluded too, because a minifier discards comments anyway.
fn is_vendored(root: &Path, path: &str) -> bool {
    if path.ends_with(".min.js") || path.ends_with(".min.css") {
        return true;
    }
    if path.starts_with("vendor/") || path.contains("/vendor/") {
        return true;
    }

    let mut directory = Path::new(path).parent();
    while let Some(current) = directory {
        if root.join(current).join("VENDOR.md").is_file() {
            return true;
        }
        directory = current.parent();
    }
    false
}

/// Everything wrong with one file's header. Empty means it is fine.
fn problems(path: &str, contents: &str, shape: Shape) -> Vec<String> {
    let mut problems = Vec::new();

    let extension = path
        .rsplit_once('.')
        .map(|(_, ext)| ext)
        .unwrap_or_default();
    let openers = CHECKED
        .iter()
        .find(|(checked, _)| *checked == extension)
        .map(|(_, openers)| *openers)
        .unwrap_or_default();

    let first = contents
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default();
    if !openers
        .iter()
        .any(|opener| first.trim_start().starts_with(opener))
    {
        problems.push(format!(
            "does not open with a comment ({}); found {:?}",
            openers.join(" or "),
            first.trim().chars().take(40).collect::<String>()
        ));
    }

    // Scanning a bounded window rather than parsing each language's comment grammar. The
    // job is to notice a missing header, and a file whose first line is a comment and
    // whose next twenty carry all six fields has one.
    let window: String = contents
        .lines()
        .take(HEADER_WINDOW)
        .collect::<Vec<_>>()
        .join("\n");

    if !window.contains("Project:") {
        problems.push("no `Project:` field".to_owned());
    }
    if !window.contains(path) {
        problems.push(format!("header does not name its own path `{path}`"));
    }
    if !window.contains("Summary:") {
        problems.push("no `Summary:` field".to_owned());
    }

    if shape == Shape::Full {
        if !window.contains("Authors:") {
            problems.push("no `Authors:` field".to_owned());
        }
        for field in ["Created:", "Modified:"] {
            match window.split_once(field) {
                None => problems.push(format!("no `{field}` field")),
                // Shape only. A mechanical check that `Modified:` matches the last commit
                // would either be wrong or fight every commit that touches the file.
                Some((_, after)) if !starts_with_iso_date(after) => {
                    problems.push(format!("`{field}` is not followed by a YYYY-MM-DD date"));
                }
                Some(_) => {}
            }
        }
    }

    problems
}

/// True when `text`, after leading spaces, begins with `YYYY-MM-DD`.
fn starts_with_iso_date(text: &str) -> bool {
    let candidate: Vec<char> = text.trim_start().chars().take(10).collect();
    if candidate.len() != 10 {
        return false;
    }
    candidate
        .iter()
        .enumerate()
        .all(|(index, character)| match index {
            4 | 7 => *character == '-',
            _ => character.is_ascii_digit(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "\
// Project:  Privatium™  |  File: crates/privatium-core/src/lib.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-08-31  |  Modified: 2026-08-31
// Summary:  Crate root.

pub fn nothing() {}
";

    #[test]
    fn a_complete_header_passes() {
        let found = problems("crates/privatium-core/src/lib.rs", GOOD, Shape::Full);
        assert!(found.is_empty(), "{found:?}");
    }

    /// The milestone's stated done-when: delete the header, and the check fails.
    #[test]
    fn a_deleted_header_fails() {
        let stripped = "pub fn nothing() {}\n";
        let found = problems("crates/privatium-core/src/lib.rs", stripped, Shape::Full);
        assert!(
            !found.is_empty(),
            "a file with no header at all was accepted"
        );
    }

    #[test]
    fn a_header_naming_the_wrong_file_fails() {
        let found = problems("crates/privatium/src/main.rs", GOOD, Shape::Full);
        assert!(
            found.iter().any(|problem| problem.contains("its own path")),
            "{found:?}"
        );
    }

    #[test]
    fn a_missing_date_fails() {
        let undated = GOOD.replace("Created:  2026-08-31", "Created:  last Tuesday");
        let found = problems("crates/privatium-core/src/lib.rs", &undated, Shape::Full);
        assert!(
            found.iter().any(|problem| problem.contains("YYYY-MM-DD")),
            "{found:?}"
        );
    }

    #[test]
    fn a_template_needs_less() {
        let template = "\
<?-- Project: Privatium™ | apps/hello/views/index.lsp
     Summary: Greeting. --?>
<h1>Hello</h1>
";
        let found = problems("apps/hello/views/index.lsp", template, Shape::Template);
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn a_template_still_needs_a_summary() {
        let template = "<?-- Project: Privatium™ | apps/hello/views/index.lsp --?>\n";
        let found = problems("apps/hello/views/index.lsp", template, Shape::Template);
        assert!(
            found.iter().any(|problem| problem.contains("Summary")),
            "{found:?}"
        );
    }

    #[test]
    fn markdown_outside_spec_and_docs_is_exempt() {
        let root = Path::new(".");
        assert!(in_scope(root, "spec/protocol.md").is_some());
        assert!(in_scope(root, "docs/icons.md").is_some());
        assert!(in_scope(root, "README.md").is_none());
        assert!(in_scope(root, "apps/hello/README.md").is_none());
        assert!(in_scope(root, "skills/README.md").is_none());
    }

    #[test]
    fn minified_bundles_are_exempt() {
        let root = Path::new(".");
        assert!(in_scope(root, "apps/animals/static/alpine-csp.min.js").is_none());
    }
}
