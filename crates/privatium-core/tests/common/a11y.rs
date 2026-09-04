// Project:  Privatium™  |  File: crates/privatium-core/tests/common/a11y.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-04  |  Modified: 2026-09-05
// Summary:  The PV4xx rules of spec/cli.md §5.1 over *rendered* HTML — the shell's pages
//           and the Tier 1 page frame, which the linter never sees because they have no
//           template (spec/cli.md §5.4). Since M12 the tree, the element checks and the
//           contrast maths are the linter's own (privatium_core::lint::{html, css}); what
//           stays here is the document-level judgement a rendered page adds — lang, one
//           main, labelled nav, the skip target, no on*=, no style=, no inline script, id
//           references that resolve — and the shape the M10 tests read.

// Each test binary uses a different subset of these re-exports, as common/mod.rs says
// of its helpers.
#![allow(unused_imports)]

use std::collections::BTreeMap;

pub use privatium_core::lint::html::{Element, Node, parse};

/// What is being checked: a whole document, or a fragment htmx swaps into one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Document,
    Fragment,
}

/// Every finding over `html`, each prefixed by the rule it breaks. Empty means clean.
pub fn check(html: &str, unit: Unit) -> Vec<String> {
    let root = parse(html);
    let mut findings = Vec::new();
    let all = root.descendants();

    // Ids, for the references below.
    let mut ids: BTreeMap<&str, usize> = BTreeMap::new();
    for element in &all {
        if let Some(id) = element.attr("id") {
            *ids.entry(id).or_default() += 1;
        }
    }
    for (id, count) in &ids {
        if *count > 1 {
            findings.push(format!("duplicate id \"{id}\" ({count} times)"));
        }
    }
    let has_id = |id: &str| ids.contains_key(id);

    if unit == Unit::Document {
        match root.find_all("html").first() {
            None => findings.push("no <html> element".into()),
            Some(html) if html.attr("lang").map(str::trim).unwrap_or("").is_empty() => {
                findings.push("<html> has no lang".into());
            }
            Some(_) => {}
        }
        match root.find_all("title").first() {
            Some(title) if !title.all_text().trim().is_empty() => {}
            _ => findings.push("no <title> with text".into()),
        }
        let mains = root.find_all("main").len();
        if mains != 1 {
            findings.push(format!("{mains} <main> landmarks, want one"));
        }
        for skip in all
            .iter()
            .filter(|e| e.name == "a" && e.has_class("pv-skip"))
        {
            match skip.attr("href").and_then(|h| h.strip_prefix('#')) {
                Some(target) if has_id(target) => {}
                other => findings.push(format!("skip link target {other:?} does not exist")),
            }
        }
    }

    for nav in root.find_all("nav") {
        if nav
            .attr("aria-label")
            .map(str::trim)
            .unwrap_or("")
            .is_empty()
            && nav.attr("aria-labelledby").is_none()
        {
            findings.push(format!("<nav> without aria-label: {}", nav.describe()));
        }
    }

    // PV404: one h1 per rendered page; no level skipped.
    let headings = privatium_core::lint::html::headings(&root);
    if unit == Unit::Document {
        let h1s = headings.iter().filter(|(l, _)| *l == 1).count();
        if h1s != 1 {
            findings.push(format!("PV404: {h1s} <h1> elements, want exactly one"));
        }
        if let Some((level, line)) = headings.first()
            && *level != 1
        {
            findings.push(format!(
                "PV404: first heading is <h{level}>, not <h1> (line {line})"
            ));
        }
    }
    for pair in headings.windows(2) {
        let ((before, _), (after, line)) = (pair[0], pair[1]);
        if after > before + 1 {
            findings.push(format!(
                "PV404: <h{before}> is followed by <h{after}> (line {line})"
            ));
        }
    }
    for element in &all {
        if element.name.len() == 2
            && element.name.starts_with('h')
            && element.name.as_bytes()[1].is_ascii_digit()
            && element.all_text().trim().is_empty()
        {
            findings.push(format!("PV404: empty heading {}", element.describe()));
        }
    }

    // PV401, PV402, PV403, PV405, PV407: the linter's element checks.
    for finding in privatium_core::lint::html::element_findings(&root) {
        findings.push(format!("{}: {}", finding.rule, finding.message));
    }

    // Hygiene the CSP would silently break: no inline handlers, no style attributes, no
    // inline script; and every id reference resolves.
    for element in &all {
        for (name, value) in &element.attrs {
            if name.starts_with("on") && name.len() > 2 {
                findings.push(format!(
                    "inline event handler {name}= on {}",
                    element.describe()
                ));
            }
            if name == "style" {
                findings.push(format!("style= attribute on {}", element.describe()));
            }
            if [
                "aria-controls",
                "aria-describedby",
                "aria-labelledby",
                "for",
            ]
            .contains(&name.as_str())
            {
                for id in value.split_whitespace() {
                    if !has_id(id) {
                        findings.push(format!(
                            "{name}=\"{id}\" names no element: {}",
                            element.describe()
                        ));
                    }
                }
            }
        }
        if element.name == "script"
            && element.attr("src").is_none()
            && !element.all_text().trim().is_empty()
        {
            findings.push("inline <script> with content".into());
        }
        if element.name == "style" {
            findings.push("inline <style> element".into());
        }
    }

    findings
}

// ---------------------------------------------------------------------------------------
// Contrast and stylesheets: the linter's, in the shape the M10 tests read
// ---------------------------------------------------------------------------------------

/// The contrast ratio between two colours, `1.0..=21.0`. Panics on a colour that is not
/// hex, which in a test is the right thing.
pub fn contrast(a: &str, b: &str) -> f64 {
    privatium_core::lint::css::contrast(a, b)
        .unwrap_or_else(|| panic!("not hex colours: {a} / {b}"))
}

pub use privatium_core::lint::css::{resolve, root_tokens};

/// Every `selector { declarations }` in the sheet, `@media` blocks flattened, in order.
pub fn rules(css: &str) -> Vec<(String, BTreeMap<String, String>)> {
    privatium_core::lint::css::rules(css)
        .into_iter()
        .map(|rule| (rule.selector, rule.declarations))
        .collect()
}
