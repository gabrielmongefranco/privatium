// Project:  Privatium™  |  File: crates/privatium-core/tests/common/a11y.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-04  |  Modified: 2026-09-04
// Summary:  The PV4xx rules of spec/cli.md §5.1 over *rendered* HTML — the shell's pages
//           and the Tier 1 page frame, which the linter of M12 never sees because they
//           have no template (spec/cli.md §5.4). A small tolerant tag scanner into a tree,
//           the element checks (PV401 names, PV402 labels, PV403 fieldsets, PV404 one h1
//           and no skipped level, PV405 status carries text, PV407 th scope) plus the
//           document ones (lang, one main, labelled nav, skip target, no on*=, no style=,
//           no inline script, id references resolve), WCAG 2.x relative luminance and
//           contrast, and a reader for a stylesheet's :root tokens and rules (PV406).

use std::collections::{BTreeMap, BTreeSet};

// ---------------------------------------------------------------------------------------
// A tolerant HTML tree
// ---------------------------------------------------------------------------------------

/// A node of the parsed document.
#[derive(Debug, Clone)]
pub enum Node {
    Element(Element),
    Text(String),
}

/// An element with its attributes (names lowercased, values as written, entities kept).
#[derive(Debug, Clone, Default)]
pub struct Element {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Node>,
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
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn has_class(&self, class: &str) -> bool {
        self.attr("class")
            .map(|c| c.split_whitespace().any(|x| x == class))
            .unwrap_or(false)
    }

    /// Every descendant element, preorder.
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

    pub fn find_all(&self, name: &str) -> Vec<&Element> {
        self.descendants()
            .into_iter()
            .filter(|e| e.name == name)
            .collect()
    }

    /// The text of every descendant, `<svg>` subtrees excluded (an icon's `<title>` is its
    /// accessible name only when the icon is `role="img"`, which [`accessible_name`] sees).
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
pub fn parse(html: &str) -> Element {
    let bytes = html.as_bytes();
    let mut stack: Vec<Element> = vec![Element {
        name: "#document".into(),
        ..Element::default()
    }];
    let mut i = 0;
    let mut text_start = 0;

    let flush_text = |stack: &mut Vec<Element>, text: &str| {
        if !text.is_empty() {
            stack
                .last_mut()
                .unwrap()
                .children
                .push(Node::Text(text.to_owned()));
        }
    };

    while i < bytes.len() {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
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
            let name = html[i + 2..end - 1].trim().to_ascii_lowercase();
            if let Some(depth) = stack.iter().rposition(|e| e.name == name) {
                while stack.len() > depth + 1 {
                    let done = stack.pop().unwrap();
                    stack.last_mut().unwrap().children.push(Node::Element(done));
                }
                if stack.len() > 1 {
                    let done = stack.pop().unwrap();
                    stack.last_mut().unwrap().children.push(Node::Element(done));
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
        let (element, self_closing, end) = open_tag(html, i);
        i = end;
        text_start = i;
        let name = element.name.clone();

        if OPTIONAL_END.contains(&name.as_str())
            && stack.last().map(|e| e.name == name).unwrap_or(false)
            && stack.len() > 1
        {
            let done = stack.pop().unwrap();
            stack.last_mut().unwrap().children.push(Node::Element(done));
        }

        if self_closing || VOID.contains(&name.as_str()) {
            stack
                .last_mut()
                .unwrap()
                .children
                .push(Node::Element(element));
            continue;
        }
        if RAW.contains(&name.as_str()) {
            let close = format!("</{name}");
            let body_end = html[i..]
                .to_ascii_lowercase()
                .find(&close)
                .map(|at| i + at)
                .unwrap_or(bytes.len());
            let mut element = element;
            if body_end > i {
                element
                    .children
                    .push(Node::Text(html[i..body_end].to_owned()));
            }
            stack
                .last_mut()
                .unwrap()
                .children
                .push(Node::Element(element));
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
        let done = stack.pop().unwrap();
        stack.last_mut().unwrap().children.push(Node::Element(done));
    }
    stack.pop().unwrap()
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
                value = html[value_start..i].to_owned();
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

// ---------------------------------------------------------------------------------------
// The checks
// ---------------------------------------------------------------------------------------

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
    let headings: Vec<(u32, &Element)> = all
        .iter()
        .filter_map(|e| {
            let level = e.name.strip_prefix('h')?.parse::<u32>().ok()?;
            (1..=6).contains(&level).then_some((level, *e))
        })
        .collect();
    if unit == Unit::Document {
        let h1s = headings.iter().filter(|(l, _)| *l == 1).count();
        if h1s != 1 {
            findings.push(format!("PV404: {h1s} <h1> elements, want exactly one"));
        }
        if let Some((level, first)) = headings.first()
            && *level != 1
        {
            findings.push(format!(
                "PV404: first heading is <h{level}>, not <h1>: {}",
                first.describe()
            ));
        }
    }
    for pair in headings.windows(2) {
        let (before, _) = pair[0];
        let (after, element) = pair[1];
        if after > before + 1 {
            findings.push(format!(
                "PV404: <h{before}> is followed by <h{after}>: {}",
                element.describe()
            ));
        }
    }
    for (_, heading) in &headings {
        if heading.all_text().trim().is_empty() {
            findings.push(format!("PV404: empty heading {}", heading.describe()));
        }
    }

    // PV401: every control has an accessible name; a decorative icon is hidden.
    for element in &all {
        let is_control = element.name == "button"
            || (element.name == "a" && element.attr("href").is_some())
            || (element.name == "input"
                && matches!(
                    element.attr("type").unwrap_or("text"),
                    "button" | "submit" | "reset" | "image"
                ));
        if is_control && !has_accessible_name(element) {
            findings.push(format!("PV401: no accessible name: {}", element.describe()));
        }
        if element.name == "svg" {
            let labelled = element.attr("role") == Some("img")
                && element
                    .find_all("title")
                    .first()
                    .map(|t| !t.all_text().trim().is_empty())
                    .unwrap_or(false);
            let hidden = element.attr("aria-hidden") == Some("true");
            if !labelled && !hidden {
                findings.push(
                    "PV401: <svg> is neither aria-hidden nor role=\"img\" with a <title>".into(),
                );
            }
            if element.attr("focusable") != Some("false") {
                findings.push("PV401: <svg> without focusable=\"false\" (docs/icons.md)".into());
            }
        }
        if element.name == "img" && element.attr("alt").is_none() {
            findings.push(format!("PV401: <img> without alt: {}", element.describe()));
        }
    }

    // PV402: every input has a <label for>; every label's for resolves.
    let labels_for: BTreeSet<&str> = all
        .iter()
        .filter(|e| e.name == "label")
        .filter_map(|e| e.attr("for"))
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
            Some(id) if labels_for.contains(id) => {}
            Some(id) => findings.push(format!(
                "PV402: no <label for=\"{id}\">: {}",
                element.describe()
            )),
            None => findings.push(format!(
                "PV402: input without an id, so no <label for>: {}",
                element.describe()
            )),
        }
    }
    for target in &labels_for {
        if !has_id(target) {
            findings.push(format!("PV402: <label for=\"{target}\"> names no element"));
        }
    }

    // PV403: radios and checkboxes sit in a fieldset with a legend.
    fn walk_groups<'a>(
        element: &'a Element,
        ancestors: &mut Vec<&'a Element>,
        findings: &mut Vec<String>,
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
                        Some(false) => findings.push(format!(
                            "PV403: fieldset without a legend around {}",
                            e.describe()
                        )),
                        None => {
                            findings.push(format!("PV403: not inside a fieldset: {}", e.describe()))
                        }
                    }
                }
                ancestors.push(e);
                walk_groups(e, ancestors, findings);
                ancestors.pop();
            }
        }
    }
    walk_groups(&root, &mut Vec::new(), &mut findings);

    // PV405: a status is words, not a colour. A `role="status"` live region may be empty
    // until something happens; an alert, a badge or a notice is never empty.
    for element in &all {
        let status_like = element.attr("role") == Some("alert")
            || element
                .attr("class")
                .map(|c| {
                    c.split_whitespace().any(|x| {
                        x.starts_with("pv-badge") || x.starts_with("pv-notice") || x == "pv-error"
                    })
                })
                .unwrap_or(false);
        if status_like && element.visible_text().trim().is_empty() {
            findings.push(format!(
                "PV405: status with no text, colour alone: {}",
                element.describe()
            ));
        }
    }

    // PV407: real tables, with scoped headers.
    for table in root.find_all("table") {
        let ths = table.find_all("th");
        if ths.is_empty() {
            findings.push("PV407: <table> without a <th>".into());
        }
        for th in ths {
            if !matches!(
                th.attr("scope"),
                Some("col" | "row" | "colgroup" | "rowgroup")
            ) {
                findings.push(format!("PV407: <th> without scope: {}", th.describe()));
            }
        }
    }
    for element in &all {
        if element.name == "div"
            && (element.has_class("row")
                || element.has_class("cell")
                || element.attr("role") == Some("grid"))
        {
            findings.push(format!("PV407: a grid of divs: {}", element.describe()));
        }
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

/// What assistive technology would announce for a control: `aria-label`, a labelled
/// element, its visible text, a labelled icon inside it, or an input's value.
fn has_accessible_name(element: &Element) -> bool {
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
    if element.name == "input" {
        return element
            .attr("value")
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false)
            || element.attr("alt").is_some();
    }
    false
}

// ---------------------------------------------------------------------------------------
// Contrast (WCAG 2.x, the sRGB formula)
// ---------------------------------------------------------------------------------------

/// `#rgb` or `#rrggbb` as sRGB channels in `0..=1`.
pub fn hex(color: &str) -> Option<(f64, f64, f64)> {
    let digits = color.trim().strip_prefix('#')?;
    let channel = |s: &str| u8::from_str_radix(s, 16).ok().map(|v| f64::from(v) / 255.0);
    match digits.len() {
        3 => {
            let expand = |c: char| format!("{c}{c}");
            let mut chars = digits.chars();
            let r = channel(&expand(chars.next()?))?;
            let g = channel(&expand(chars.next()?))?;
            let b = channel(&expand(chars.next()?))?;
            Some((r, g, b))
        }
        6 => Some((
            channel(&digits[0..2])?,
            channel(&digits[2..4])?,
            channel(&digits[4..6])?,
        )),
        _ => None,
    }
}

/// Relative luminance.
pub fn luminance(color: &str) -> f64 {
    let (r, g, b) = hex(color).unwrap_or_else(|| panic!("not a hex colour: {color}"));
    let lin = |c: f64| {
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

/// The contrast ratio between two colours, `1.0..=21.0`.
pub fn contrast(a: &str, b: &str) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

// ---------------------------------------------------------------------------------------
// A stylesheet's tokens and rules
// ---------------------------------------------------------------------------------------

/// The custom properties of every `:root { … }` block, in order — the light scheme
/// first, then the one inside `@media (prefers-color-scheme: dark)`.
pub fn root_tokens(css: &str) -> Vec<BTreeMap<String, String>> {
    let css = strip_comments(css);
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(at) = css[from..].find(":root") {
        let start = from + at;
        let Some(open) = css[start..].find('{') else {
            break;
        };
        let Some(close) = css[start + open..].find('}') else {
            break;
        };
        let block = &css[start + open + 1..start + open + close];
        out.push(declarations(block));
        from = start + open + close + 1;
    }
    out
}

/// Every `selector { declarations }` in the sheet, `@media` blocks flattened, in order.
pub fn rules(css: &str) -> Vec<(String, BTreeMap<String, String>)> {
    let css = strip_comments(css);
    let mut out = Vec::new();
    collect_rules(&css, &mut out);
    out
}

fn collect_rules(css: &str, into: &mut Vec<(String, BTreeMap<String, String>)>) {
    let mut rest = css;
    while let Some(open) = rest.find('{') {
        let selector = rest[..open].trim().to_owned();
        let body_start = open + 1;
        // Find the matching close brace.
        let mut depth = 1usize;
        let mut end = None;
        for (offset, ch) in rest[body_start..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(body_start + offset);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(end) = end else {
            break;
        };
        let body = &rest[body_start..end];
        if selector.starts_with('@') && !selector.starts_with("@font-face") {
            collect_rules(body, into);
        } else {
            for one in selector.split(',') {
                into.push((one.trim().to_owned(), declarations(body)));
            }
        }
        rest = &rest[end + 1..];
    }
}

fn declarations(block: &str) -> BTreeMap<String, String> {
    block
        .split(';')
        .filter_map(|decl| {
            let (name, value) = decl.split_once(':')?;
            let name = name.trim();
            if name.is_empty() {
                return None;
            }
            Some((name.to_owned(), value.trim().to_owned()))
        })
        .collect()
}

fn strip_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        match rest[start..].find("*/") {
            Some(end) => rest = &rest[start + end + 2..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// A declaration's value with `var(--x)` resolved through `tokens`, or the literal.
pub fn resolve<'a>(value: &'a str, tokens: &'a BTreeMap<String, String>) -> Option<&'a str> {
    let value = value.trim();
    if let Some(inner) = value.strip_prefix("var(").and_then(|v| v.strip_suffix(')')) {
        return tokens.get(inner.trim()).map(String::as_str);
    }
    Some(value)
}
