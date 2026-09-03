// Project:  Privatium™  |  File: crates/privatium-core/src/icons.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-03  |  Modified: 2026-09-03
// Summary:  Bootstrap Icons, vendored in full under assets/icons/ and embedded in the binary
//           (docs/icons.md). The one helper that turns a name into an inline <svg>, with
//           the accessibility attributes the icon system makes mandatory and the fallback
//           an unknown name renders instead of nothing.

use include_dir::{Dir, include_dir};

/// Every `.svg` of `twbs/icons` at the pinned tag, plus `LICENSE`, `VERSION` and
/// `VENDOR.md`. Not a subset: apps declare their own icon at runtime (`docs/icons.md`).
static ICONS: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/assets/icons");

/// What an unknown name renders as. It never renders nothing (`docs/icons.md`).
pub const FALLBACK: &str = "question-circle";

/// The vendored tag, from `assets/icons/VERSION` — `v1.13.1` as `docs/icons.md` pins it.
#[must_use]
pub fn version() -> &'static str {
    ICONS
        .get_file("VERSION")
        .and_then(|file| file.contents_utf8())
        .map_or("", str::trim)
}

/// How many icons the set holds.
#[must_use]
pub fn count() -> usize {
    ICONS
        .files()
        .filter(|file| file.path().extension().is_some_and(|ext| ext == "svg"))
        .count()
}

/// Whether `name` is a well-formed icon name (`^[a-z0-9-]+$`) that exists in the set.
#[must_use]
pub fn exists(name: &str) -> bool {
    body(name).is_some()
}

/// A decorative icon: `aria-hidden="true"`, `focusable="false"`, `1em` square,
/// `currentColor`. For an icon that sits beside its text.
///
/// An unknown or malformed name renders [`FALLBACK`]. Nothing here logs — the shell is
/// what knows an owner is looking, and it surfaces a load-report warning for an app whose
/// manifest names an icon the set lacks.
#[must_use]
pub fn icon(name: &str) -> String {
    render(name, None)
}

/// An icon that is the only content of a control: `role="img"` with a `<title>` carrying
/// `label`, which assistive technology announces. An icon-only button without one is an
/// accessibility bug (`docs/icons.md`).
#[must_use]
pub fn icon_labeled(name: &str, label: &str) -> String {
    render(name, Some(label))
}

fn render(name: &str, label: Option<&str>) -> String {
    let inner = body(name).or_else(|| body(FALLBACK)).unwrap_or_default();
    let mut out = String::with_capacity(inner.len() + 160);
    out.push_str(
        r#"<svg class="pv-icon" width="1em" height="1em" fill="currentColor" viewBox="0 0 16 16""#,
    );
    match label {
        Some(label) => {
            out.push_str(r#" role="img" focusable="false"><title>"#);
            escape_into(&mut out, label);
            out.push_str("</title>");
        }
        None => out.push_str(r#" aria-hidden="true" focusable="false">"#),
    }
    out.push_str(inner);
    out.push_str("</svg>");
    out
}

/// The children of the vendored `<svg>` — its `<path>` elements — as text.
///
/// Every file in the set has the shape `<svg …>…</svg>`; the opening tag is discarded
/// because ours carries different attributes, and the closing one because we write it.
fn body(name: &str) -> Option<&'static str> {
    if name.is_empty()
        || !name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return None;
    }
    let svg = ICONS.get_file(format!("{name}.svg"))?.contents_utf8()?;
    let start = svg.find("<svg")?;
    let open_end = start + svg[start..].find('>')? + 1;
    let close = svg.rfind("</svg>")?;
    (open_end <= close).then(|| svg[open_end..close].trim())
}

/// Minimal HTML escaping for text content and attribute values.
pub(crate) fn escape_into(out: &mut String, text: &str) {
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
}

/// [`escape_into`], returning a new string. Public so a test can spell what a page shows.
#[must_use]
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    escape_into(&mut out, text);
    out
}

// AGENTS.md, Style: unwrap() is permitted in tests. The crate-level deny reaches unit
// tests inside src/, so each one opts out where it is declared.
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pinned_version_is_vendored_in_full() {
        assert_eq!(version(), "v1.13.1");
        assert!(count() > 2000, "{}", count());
        assert!(ICONS.get_file("LICENSE").is_some());
        assert!(ICONS.get_file("VENDOR.md").is_some());
    }

    /// `docs/icons.md`, "Rules the helper enforces".
    #[test]
    fn a_decorative_icon_is_hidden_and_unfocusable() {
        let svg = icon("gear");
        assert!(svg.starts_with("<svg class=\"pv-icon\" width=\"1em\" height=\"1em\""));
        assert!(svg.contains(r#"fill="currentColor""#));
        assert!(svg.contains(r#"viewBox="0 0 16 16""#));
        assert!(svg.contains(r#"aria-hidden="true""#));
        assert!(svg.contains(r#"focusable="false""#));
        assert!(svg.contains("<path"));
        assert!(svg.ends_with("</svg>"));
        assert!(
            !svg.contains("class=\"bi"),
            "the vendored opening tag leaked: {svg}"
        );
    }

    #[test]
    fn a_labelled_icon_is_an_image_with_a_title() {
        let svg = icon_labeled("trash", "Delete <this>");
        assert!(svg.contains(r#"role="img""#));
        assert!(svg.contains("<title>Delete &lt;this&gt;</title>"));
        assert!(!svg.contains("aria-hidden"));
        assert!(svg.contains(r#"focusable="false""#));
    }

    /// An unknown name renders `question-circle` and never nothing.
    #[test]
    fn an_unknown_name_renders_the_fallback() {
        // Named through variables so `cargo xtask icons-verify`, which scans for literal calls
        // with a quoted name, does not read these as the shell asking for them.
        let missing = "no-such-icon-ever";
        let traversal = "../LICENSE";
        let fallback = icon(FALLBACK);
        assert_eq!(icon(missing), fallback);
        assert_eq!(icon(traversal), fallback);
        assert_eq!(icon(""), fallback);
        assert!(!exists(missing));
        assert!(exists("gear"));
    }

    /// Every name in `docs/icons.md`'s vocabulary table exists — the build fails rather
    /// than falling back silently, as that document asks.
    #[test]
    fn the_framework_vocabulary_exists() {
        for name in [
            "plus-lg",
            "pencil",
            "trash",
            "check-lg",
            "x-lg",
            "gear",
            "phone",
            "qr-code",
            "arrow-repeat",
            "archive",
            "exclamation-triangle",
            "shield-exclamation",
            "info-circle",
            "search",
            "grid-3x3-gap",
            "question-circle",
        ] {
            assert!(exists(name), "{name} is not in the vendored set");
        }
    }
}
