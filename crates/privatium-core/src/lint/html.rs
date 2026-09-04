// Project:  Privatium™  |  File: crates/privatium-core/src/lint/html.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-05  |  Modified: 2026-09-05
// Summary:  A small tolerant HTML tree — lifted from tests/common/a11y.rs, where M10 first
//           wrote it for the shell's rendered pages — and the element-level checks of the
//           PV4xx rules (spec/cli.md §5.1) that a template, a Tier 2 document and a
//           rendered page all answer to: PV401 names, PV402 labels, PV403 fieldsets, PV405
//           status text, PV407 real tables. Each element carries the line it opened on, so
//           a finding names it; the template layer keeps its synthesized HTML line-aligned
//           with the .lsp source, which is what makes that line the author's.

use std::collections::{BTreeMap, BTreeSet};

use crate::lint::RuleId;

/// A node of the parsed document.
#[derive(Debug, Clone)]
pub enum Node {
    /// An element.
    Element(Element),
    /// Text between elements, entities kept.
    Text(String),
}

/// An element with its attributes (names lowercased, values as written, entities kept).
#[derive(Debug, Clone, Default)]
pub struct Element {
    /// The tag name, lowercased.
    pub name: String,
    /// Attributes in document order.
    pub attrs: Vec<(String, String)>,
    /// Children in document order.
    pub children: Vec<Node>,
    /// 1-based line the open tag is on.
    pub line: u32,
}

/// Elements that never have content or an end tag.
const VOID: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

/// Elements whose content is raw text up to the end tag.
const RAW: &[&str] = &["script", "style"];

/// Elements whose end tag may be omitted: a new one closes an open one.
const OPTIONAL_END: &[&str] = &["p", "li", "dt", "dd", "tr", "td", "th", "option"];

impl Element {
    /// The value of attribute `name`, if present.
    #[must_use]
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    /// Whether `class` is one of the element's classes.
    #[must_use]
    pub fn has_class(&self, class: &str) -> bool {
        self.attr("class")
            .map(|c| c.split_whitespace().any(|x| x == class))
            .unwrap_or(false)
    }

    /// Every descendant element, preorder.
    #[must_use]
    pub fn descendants(&self) -> Vec<&Element> {
        let mut out = Vec::new();
        fn walk<'a>(node: &'a Element, into: &mut Vec<&'a Element>) {
            for child in &node.children {
                if let Node::Element(element) = child {
                    into.push(element);
                    walk(element, into);
                }
            }
        }
        walk(self, &mut out);
        out
    }

    /// Every descendant named `name`.
    #[must_use]
    pub fn find_all(&self, name: &str) -> Vec<&Element> {
        self.descendants()
            .into_iter()
            .filter(|e| e.name == name)
            .collect()
    }

    /// The text of every descendant, `<svg>` subtrees excluded (an icon's `<title>` is its
    /// accessible name only when the icon is `role="img"`, which [`has_accessible_name`]
    /// sees).
    #[must_use]
    pub fn visible_text(&self) -> String {
        let mut out = String::new();
        fn walk(node: &Element, into: &mut String) {
            for child in &node.children {
                match child {
                    Node::Text(text) => into.push_str(text),
                    Node::Element(element) if element.name == "svg" => {}
                    Node::Element(element) => walk(element, into),
                }
            }
        }
        walk(self, &mut out);
        out
    }

    /// All the text, including `<svg><title>`.
    #[must_use]
    pub fn all_text(&self) -> String {
        let mut out = String::new();
        fn walk(node: &Element, into: &mut String) {
            for child in &node.children {
                match child {
                    Node::Text(text) => into.push_str(text),
                    Node::Element(element) => walk(element, into),
                }
            }
        }
        walk(self, &mut out);
        out
    }

    /// A one-line description for a finding.
    #[must_use]
    pub fn describe(&self) -> String {
        let mut out = format!("<{}", self.name);
        for (name, value) in &self.attrs {
            if ["id", "class", "name", "href", "type", "role", "aria-label"]
                .contains(&name.as_str())
            {
                out.push_str(&format!(" {name}=\"{value}\""));
            }
        }
        out.push('>');
        let text: String = self
            .visible_text()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if !text.is_empty() {
            let short: String = text.chars().take(40).collect();
            out.push_str(&short);
            if text.chars().count() > 40 {
                out.push('…');
            }
        }
        out
    }
}

/// Parse `html` into a tree under a synthetic `#document` element. Comments and the
/// doctype are dropped, `<script>`/`<style>` contents kept as text, void elements and
/// `/>` never opened, an end tag with no matching open element ignored.
#[must_use]
pub fn parse(html: &str) -> Element {
    let bytes = html.as_bytes();
    let mut stack: Vec<Element> = vec![Element {
        name: "#document".into(),
        line: 1,
        ..Element::default()
    }];
    let mut i = 0;
    let mut text_start = 0;
    let mut line = 1u32;
    let mut line_at = 0usize;

    fn flush_text(stack: &mut [Element], text: &str) {
        if !text.is_empty()
            && let Some(top) = stack.last_mut()
        {
            top.children.push(Node::Text(text.to_owned()));
        }
    }
    fn close(stack: &mut Vec<Element>) {
        if stack.len() > 1
            && let Some(done) = stack.pop()
            && let Some(top) = stack.last_mut()
        {
            top.children.push(Node::Element(done));
        }
    }

    while i < bytes.len() {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        // The line of this `<`, counted from where the last count stopped.
        line += html[line_at..i].bytes().filter(|b| *b == b'\n').count() as u32;
        line_at = i;
        let rest = &html[i..];
        if rest.starts_with("<!--") {
            flush_text(&mut stack, &html[text_start..i]);
            let end = rest.find("-->").map(|at| i + at + 3).unwrap_or(bytes.len());
            i = end;
            text_start = i;
            continue;
        }
        if rest.starts_with("<!") || rest.starts_with("<?") {
            flush_text(&mut stack, &html[text_start..i]);
            let end = rest.find('>').map(|at| i + at + 1).unwrap_or(bytes.len());
            i = end;
            text_start = i;
            continue;
        }
        if rest.starts_with("</") {
            flush_text(&mut stack, &html[text_start..i]);
            let end = rest.find('>').map(|at| i + at + 1).unwrap_or(bytes.len());
            let name = html[i + 2..end.saturating_sub(1).max(i + 2)]
                .trim()
                .to_ascii_lowercase();
            if let Some(depth) = stack.iter().rposition(|e| e.name == name) {
                while stack.len() > depth + 1 {
                    close(&mut stack);
                }
                if depth > 0 {
                    close(&mut stack);
                }
            }
            i = end;
            text_start = i;
            continue;
        }
        // An open tag, if what follows is a name; otherwise a literal `<`.
        let after = rest[1..].chars().next().unwrap_or(' ');
        if !after.is_ascii_alphabetic() {
            i += 1;
            continue;
        }
        flush_text(&mut stack, &html[text_start..i]);
        let (mut element, self_closing, end) = open_tag(html, i);
        element.line = line;
        i = end;
        text_start = i;
        let name = element.name.clone();

        if OPTIONAL_END.contains(&name.as_str())
            && stack.last().map(|e| e.name == name).unwrap_or(false)
            && stack.len() > 1
        {
            close(&mut stack);
        }

        if self_closing || VOID.contains(&name.as_str()) {
            if let Some(top) = stack.last_mut() {
                top.children.push(Node::Element(element));
            }
            continue;
        }
        if RAW.contains(&name.as_str()) {
            let close_tag = format!("</{name}");
            let body_end = html[i..]
                .to_ascii_lowercase()
                .find(&close_tag)
                .map(|at| i + at)
                .unwrap_or(bytes.len());
            let mut element = element;
            if body_end > i {
                element
                    .children
                    .push(Node::Text(html[i..body_end].to_owned()));
            }
            if let Some(top) = stack.last_mut() {
                top.children.push(Node::Element(element));
            }
            let end = html[body_end..]
                .find('>')
                .map(|at| body_end + at + 1)
                .unwrap_or(bytes.len());
            i = end;
            text_start = i;
            continue;
        }
        stack.push(element);
    }
    flush_text(&mut stack, &html[text_start..]);
    while stack.len() > 1 {
        close(&mut stack);
    }
    stack.pop().unwrap_or_default()
}

/// Read an open tag starting at `html[at] == '<'`: the element, whether it ended in `/>`,
/// and the index past `>`.
fn open_tag(html: &str, at: usize) -> (Element, bool, usize) {
    let bytes = html.as_bytes();
    let mut i = at + 1;
    let name_start = i;
    while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'>' && bytes[i] != b'/'
    {
        i += 1;
    }
    let mut element = Element {
        name: html[name_start..i].to_ascii_lowercase(),
        ..Element::default()
    };
    let mut self_closing = false;
    loop {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            return (element, self_closing, i);
        }
        if bytes[i] == b'>' {
            return (element, self_closing, i + 1);
        }
        if bytes[i] == b'/' {
            self_closing = true;
            i += 1;
            continue;
        }
        let attr_start = i;
        while i < bytes.len()
            && !bytes[i].is_ascii_whitespace()
            && bytes[i] != b'='
            && bytes[i] != b'>'
            && bytes[i] != b'/'
        {
            i += 1;
        }
        let name = html[attr_start..i].to_ascii_lowercase();
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let mut value = String::new();
        if i < bytes.len() && bytes[i] == b'=' {
            i += 1;
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < bytes.len() && (bytes[i] == b'"' || bytes[i] == b'\'') {
                let quote = bytes[i];
                let value_start = i + 1;
                i += 1;
                while i < bytes.len() && bytes[i] != quote {
                    i += 1;
                }
                value = html[value_start..i.min(bytes.len())].to_owned();
                i += 1;
            } else {
                let value_start = i;
                while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'>' {
                    i += 1;
                }
                value = html[value_start..i].to_owned();
            }
        }
        if !name.is_empty() {
            element.attrs.push((name, value));
        }
    }
}

/// What assistive technology would announce for a control: `aria-label`, a labelled
/// element, its visible text, a labelled icon inside it, or an input's value.
#[must_use]
pub fn has_accessible_name(element: &Element) -> bool {
    if element
        .attr("aria-label")
        .map(str::trim)
        .map(|l| !l.is_empty())
        .unwrap_or(false)
        || element.attr("aria-labelledby").is_some()
        || element
            .attr("title")
            .map(|t| !t.trim().is_empty())
            .unwrap_or(false)
    {
        return true;
    }
    if !element.visible_text().trim().is_empty() {
        return true;
    }
    if element.find_all("svg").iter().any(|svg| {
        svg.attr("role") == Some("img")
            && svg
                .find_all("title")
                .first()
                .map(|t| !t.all_text().trim().is_empty())
                .unwrap_or(false)
    }) {
        return true;
    }
    if element.name == "img"
        || element
            .find_all("img")
            .iter()
            .any(|img| img.attr("alt").is_some_and(|a| !a.trim().is_empty()))
    {
        return element.name != "img" || element.attr("alt").is_some_and(|a| !a.trim().is_empty());
    }
    if element.name == "input" {
        return element
            .attr("value")
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false)
            || element.attr("alt").is_some();
    }
    false
}

/// One element-level finding: the rule, the line, what is wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElementFinding {
    /// The rule.
    pub rule: RuleId,
    /// 1-based line of the element.
    pub line: u32,
    /// What is wrong, in one sentence.
    pub message: String,
    /// Whether a mechanical fix exists — `focusable="false"` on an inline `<svg>`
    /// (`spec/cli.md §5.3`).
    pub fixable: bool,
}

/// Whether an `<svg>` is decorative or labelled the way `docs/icons.md` requires.
fn svg_labelled_or_hidden(svg: &Element) -> bool {
    let labelled = svg.attr("role") == Some("img")
        && svg
            .find_all("title")
            .first()
            .map(|t| !t.all_text().trim().is_empty())
            .unwrap_or(false);
    labelled || svg.attr("aria-hidden") == Some("true")
}

/// The element checks of `PV401`, `PV402`, `PV403`, `PV405` and `PV407` over a tree.
/// Heading order (`PV404`) is not here: its unit is the rendered page, and the template
/// layer judges it over branches.
#[must_use]
pub fn element_findings(root: &Element) -> Vec<ElementFinding> {
    let mut findings = Vec::new();
    let all = root.descendants();

    let ids: BTreeSet<&str> = all.iter().filter_map(|e| e.attr("id")).collect();

    // PV401: every control has an accessible name; an icon is decorative or labelled.
    for element in &all {
        let is_control = element.name == "button"
            || (element.name == "a" && element.attr("href").is_some())
            || (element.name == "input"
                && matches!(
                    element.attr("type").unwrap_or("text"),
                    "button" | "submit" | "reset" | "image"
                ));
        if is_control && !has_accessible_name(element) {
            let icon_only = !element.find_all("svg").is_empty();
            findings.push(ElementFinding {
                rule: RuleId::PV401,
                line: element.line,
                message: if icon_only {
                    format!(
                        "icon-only control with no label: {} — pass the label as icon()'s \
                         second argument, or add aria-label",
                        element.describe()
                    )
                } else {
                    format!("control with no accessible name: {}", element.describe())
                },
                fixable: false,
            });
        }
        if element.name == "svg" {
            if !svg_labelled_or_hidden(element) {
                findings.push(ElementFinding {
                    rule: RuleId::PV401,
                    line: element.line,
                    message: "<svg> is neither aria-hidden=\"true\" nor role=\"img\" with a \
                              <title> (docs/icons.md)"
                        .into(),
                    fixable: false,
                });
            }
            if element.attr("focusable") != Some("false") {
                findings.push(ElementFinding {
                    rule: RuleId::PV401,
                    line: element.line,
                    message: "<svg> without focusable=\"false\": older browsers put it in the tab \
                              order (docs/icons.md)"
                        .into(),
                    fixable: true,
                });
            }
        }
        if element.name == "img" && element.attr("alt").is_none() {
            findings.push(ElementFinding {
                rule: RuleId::PV401,
                line: element.line,
                message: format!("<img> without alt: {}", element.describe()),
                fixable: false,
            });
        }
    }

    // PV402: every input has a <label for>; every label's for resolves.
    let labels_for: BTreeMap<&str, u32> = all
        .iter()
        .filter(|e| e.name == "label")
        .filter_map(|e| e.attr("for").map(|f| (f, e.line)))
        .collect();
    for element in &all {
        let labelled_kind = element.name == "select"
            || element.name == "textarea"
            || (element.name == "input"
                && !matches!(
                    element.attr("type").unwrap_or("text"),
                    "hidden" | "submit" | "button" | "reset" | "image"
                ));
        if !labelled_kind {
            continue;
        }
        match element.attr("id") {
            Some(id) if labels_for.contains_key(id) => {}
            Some(id) => findings.push(ElementFinding {
                rule: RuleId::PV402,
                line: element.line,
                message: format!("no <label for=\"{id}\"> for {}", element.describe()),
                fixable: false,
            }),
            None => findings.push(ElementFinding {
                rule: RuleId::PV402,
                line: element.line,
                message: format!(
                    "input without an id, so no <label for> can name it: {}",
                    element.describe()
                ),
                fixable: false,
            }),
        }
    }
    for (target, line) in &labels_for {
        if !ids.contains(target) {
            findings.push(ElementFinding {
                rule: RuleId::PV402,
                line: *line,
                message: format!("<label for=\"{target}\"> names no element"),
                fixable: false,
            });
        }
    }

    // PV403: radios and checkboxes sit in a fieldset with a legend.
    fn walk_groups<'a>(
        element: &'a Element,
        ancestors: &mut Vec<&'a Element>,
        findings: &mut Vec<ElementFinding>,
    ) {
        for child in &element.children {
            if let Node::Element(e) = child {
                if e.name == "input" && matches!(e.attr("type"), Some("radio" | "checkbox")) {
                    let fieldset = ancestors.iter().rev().find(|a| a.name == "fieldset");
                    let legend = fieldset.map(|f| {
                        f.children.iter().any(|c| {
                            matches!(c, Node::Element(l) if l.name == "legend" && !l.all_text().trim().is_empty())
                        })
                    });
                    match legend {
                        Some(true) => {}
                        Some(false) => findings.push(ElementFinding {
                            rule: RuleId::PV403,
                            line: e.line,
                            message: format!("fieldset without a legend around {}", e.describe()),
                            fixable: false,
                        }),
                        None => findings.push(ElementFinding {
                            rule: RuleId::PV403,
                            line: e.line,
                            message: format!(
                                "not inside a fieldset with a legend: {}",
                                e.describe()
                            ),
                            fixable: false,
                        }),
                    }
                }
                ancestors.push(e);
                walk_groups(e, ancestors, findings);
                ancestors.pop();
            }
        }
    }
    walk_groups(root, &mut Vec::new(), &mut findings);

    // PV405: a status is words, not a colour. A `role="status"` live region may be empty
    // until something happens; an alert, a badge, a notice or a coloured dot is never
    // empty.
    for element in &all {
        let status_like = element.attr("role") == Some("alert")
            || element
                .attr("class")
                .map(|c| {
                    c.split_whitespace().any(|x| {
                        x.starts_with("pv-badge")
                            || x.starts_with("pv-notice")
                            || x == "pv-error"
                            || COLOUR_CLASSES.contains(&x)
                    })
                })
                .unwrap_or(false);
        if status_like
            && element.visible_text().trim().is_empty()
            && !has_accessible_name(element)
            && element
                .find_all("svg")
                .iter()
                .all(|s| !svg_labelled_or_hidden(s) || s.attr("role") != Some("img"))
        {
            findings.push(ElementFinding {
                rule: RuleId::PV405,
                line: element.line,
                message: format!(
                    "status with no text, so colour alone carries it: {}",
                    element.describe()
                ),
                fixable: false,
            });
        }
    }

    // PV407: real tables, with scoped headers.
    for table in root.find_all("table") {
        let ths = table.find_all("th");
        if ths.is_empty() {
            findings.push(ElementFinding {
                rule: RuleId::PV407,
                line: table.line,
                message: "<table> without a <th>".into(),
                fixable: false,
            });
        }
        for th in ths {
            if !matches!(
                th.attr("scope"),
                Some("col" | "row" | "colgroup" | "rowgroup")
            ) {
                findings.push(ElementFinding {
                    rule: RuleId::PV407,
                    line: th.line,
                    message: format!("<th> without scope: {}", th.describe()),
                    fixable: false,
                });
            }
        }
    }
    for element in &all {
        if element.name == "div"
            && (element.has_class("row")
                || element.has_class("cell")
                || element.attr("role") == Some("grid"))
        {
            findings.push(ElementFinding {
                rule: RuleId::PV407,
                line: element.line,
                message: format!(
                    "a grid of divs where tabular data wants a <table>: {}",
                    element.describe()
                ),
                fixable: false,
            });
        }
    }

    findings
}

/// Class names that name a colour and nothing else — the `<span class="dot red">` of the
/// accessibility skill's anti-pattern.
const COLOUR_CLASSES: &[&str] = &[
    "red", "green", "yellow", "orange", "amber", "blue", "grey", "gray", "success", "danger",
    "warning", "ok", "bad",
];

/// The heading levels of a tree in document order, with their lines.
#[must_use]
pub fn headings(root: &Element) -> Vec<(u32, u32)> {
    root.descendants()
        .into_iter()
        .filter_map(|e| {
            let level = e.name.strip_prefix('h')?.parse::<u32>().ok()?;
            (1..=6).contains(&level).then_some((level, e.line))
        })
        .collect()
}

// AGENTS.md, Style: unwrap() is permitted in tests. The crate-level deny reaches unit
// tests inside src/, so each one opts out where it is declared.
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tree_keeps_lines_and_nesting() {
        let root = parse("<div>\n<p>a<b>x</b>\n<p>b</div>\n<input id=\"x\">");
        let ps = root.find_all("p");
        assert_eq!(ps.len(), 2);
        assert_eq!(ps[0].line, 2);
        assert_eq!(ps[1].line, 3);
        assert_eq!(root.find_all("input")[0].line, 4);
        assert_eq!(root.find_all("div")[0].find_all("p").len(), 2);
    }

    #[test]
    fn the_element_rules_fire_and_pass() {
        let bad = parse(
            "<button><svg></svg></button>\n<input name=\"a\">\n<input type=\"radio\">\n\
             <span class=\"dot red\"></span>\n<table><tr><td>x</td></tr></table>",
        );
        let rules: Vec<RuleId> = element_findings(&bad).iter().map(|f| f.rule).collect();
        for rule in [
            RuleId::PV401,
            RuleId::PV402,
            RuleId::PV403,
            RuleId::PV405,
            RuleId::PV407,
        ] {
            assert!(rules.contains(&rule), "{rule:?} missing from {rules:?}");
        }
        let good = parse(
            "<button aria-label=\"Go\"><svg aria-hidden=\"true\" focusable=\"false\"></svg></button>\n\
             <label for=\"a\">A</label><input id=\"a\" name=\"a\">\n\
             <fieldset><legend>Pick</legend><input type=\"radio\" id=\"r\"><label for=\"r\">R</label></fieldset>\n\
             <span class=\"dot red\">Overdue</span>\n\
             <table><tr><th scope=\"col\">x</th></tr></table>",
        );
        assert!(
            element_findings(&good).is_empty(),
            "{:?}",
            element_findings(&good)
        );
    }
}
