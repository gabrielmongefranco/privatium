// Project:  Privatium™  |  File: crates/privatium-core/src/lint/css.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-05  |  Modified: 2026-09-05
// Summary:  What PV406 (spec/cli.md §5.1) reads: a stylesheet's `:root` tokens and rules,
//           `var()` resolution, and the WCAG 2.x relative-luminance contrast ratio — the
//           maths M10 wrote in tests/common/a11y.rs for the shell's own stylesheet, lifted
//           here so an app's declared tokens are held to the same floors. Nothing here
//           panics on a colour it cannot read; an unreadable value is simply not checked.

use std::collections::BTreeMap;

/// `#rgb` or `#rrggbb` as sRGB channels in `0..=1`; a small set of named colours too.
#[must_use]
pub fn hex(color: &str) -> Option<(f64, f64, f64)> {
    let color = color.trim();
    let named = match color.to_ascii_lowercase().as_str() {
        "white" => Some("#ffffff"),
        "black" => Some("#000000"),
        "red" => Some("#ff0000"),
        "green" => Some("#008000"),
        "blue" => Some("#0000ff"),
        "yellow" => Some("#ffff00"),
        "gray" | "grey" => Some("#808080"),
        "silver" => Some("#c0c0c0"),
        "navy" => Some("#000080"),
        "orange" => Some("#ffa500"),
        _ => None,
    };
    let digits = named.unwrap_or(color).strip_prefix('#')?;
    let channel = |s: &str| u8::from_str_radix(s, 16).ok().map(|v| f64::from(v) / 255.0);
    match digits.len() {
        3 | 4 => {
            let expand = |c: char| format!("{c}{c}");
            let mut chars = digits.chars();
            let r = channel(&expand(chars.next()?))?;
            let g = channel(&expand(chars.next()?))?;
            let b = channel(&expand(chars.next()?))?;
            Some((r, g, b))
        }
        6 | 8 => Some((
            channel(&digits[0..2])?,
            channel(&digits[2..4])?,
            channel(&digits[4..6])?,
        )),
        _ => None,
    }
}

/// Relative luminance, or `None` for a colour [`hex`] cannot read.
#[must_use]
pub fn luminance(color: &str) -> Option<f64> {
    let (r, g, b) = hex(color)?;
    let lin = |c: f64| {
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    Some(0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b))
}

/// The contrast ratio between two colours, `1.0..=21.0`, or `None` if either is unreadable.
#[must_use]
pub fn contrast(a: &str, b: &str) -> Option<f64> {
    let (la, lb) = (luminance(a)?, luminance(b)?);
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    Some((hi + 0.05) / (lo + 0.05))
}

/// The custom properties of every `:root { … }` block, in order — the light scheme
/// first, then the one inside `@media (prefers-color-scheme: dark)`.
#[must_use]
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

/// Every `selector { declarations }` in the sheet, `@media` blocks flattened, in order,
/// with the line each rule starts on.
#[must_use]
pub fn rules(css: &str) -> Vec<Rule> {
    let css = strip_comments(css);
    let mut out = Vec::new();
    collect_rules(&css, 0, &mut out);
    out
}

/// One style rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    /// The selector, one per comma-separated part.
    pub selector: String,
    /// The declarations, by property.
    pub declarations: BTreeMap<String, String>,
    /// 1-based line of the selector.
    pub line: u32,
}

fn collect_rules(css: &str, base: usize, into: &mut Vec<Rule>) {
    let mut rest = css;
    let mut offset = base;
    while let Some(open) = rest.find('{') {
        let selector = rest[..open].trim().to_owned();
        let selector_at = offset + open - rest[..open].trim_start().len().min(open);
        let body_start = open + 1;
        let mut depth = 1usize;
        let mut end = None;
        for (at, ch) in rest[body_start..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(body_start + at);
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
            collect_rules(body, offset + body_start, into);
        } else {
            let line = line_of(css, selector_at.saturating_sub(base));
            for one in selector.split(',') {
                into.push(Rule {
                    selector: one.trim().to_owned(),
                    declarations: declarations(body),
                    line,
                });
            }
        }
        offset += end + 1;
        rest = &rest[end + 1..];
    }
}

fn line_of(text: &str, offset: usize) -> u32 {
    text.bytes()
        .take(offset.min(text.len()))
        .filter(|b| *b == b'\n')
        .count() as u32
        + 1
}

/// The declarations of one block, by property (lowercased).
#[must_use]
pub fn declarations(block: &str) -> BTreeMap<String, String> {
    block
        .split(';')
        .filter_map(|decl| {
            let (name, value) = decl.split_once(':')?;
            let name = name.trim();
            if name.is_empty() {
                return None;
            }
            Some((name.to_ascii_lowercase(), value.trim().to_owned()))
        })
        .collect()
}

/// `css` with every `/* … */` replaced by spaces of the same shape, so lines hold.
#[must_use]
pub fn strip_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        match rest[start..].find("*/") {
            Some(end) => {
                for ch in rest[start..start + end + 2].chars() {
                    out.push(if ch == '\n' { '\n' } else { ' ' });
                }
                rest = &rest[start + end + 2..];
            }
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// A declaration's value with `var(--x)` resolved through `tokens`, or the literal.
#[must_use]
pub fn resolve<'a>(value: &'a str, tokens: &'a BTreeMap<String, String>) -> Option<&'a str> {
    let value = value.trim();
    if let Some(inner) = value.strip_prefix("var(").and_then(|v| v.strip_suffix(')')) {
        let name = inner.split(',').next().unwrap_or(inner).trim();
        return tokens.get(name).map(String::as_str);
    }
    Some(value)
}

/// The first colour in a shorthand such as `1px solid #abc` or `3px solid var(--x)`,
/// resolved through `tokens`.
#[must_use]
pub fn colour_in<'a>(value: &'a str, tokens: &'a BTreeMap<String, String>) -> Option<&'a str> {
    value
        .split_whitespace()
        .filter_map(|part| resolve(part, tokens))
        .find(|part| hex(part).is_some())
}

// AGENTS.md, Style: unwrap() is permitted in tests. The crate-level deny reaches unit
// tests inside src/, so each one opts out where it is declared.
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contrast_is_wcag() {
        assert!((contrast("#000", "#fff").unwrap() - 21.0).abs() < 1e-9);
        assert!((contrast("#ffffff", "#ffffff").unwrap() - 1.0).abs() < 1e-9);
        assert!(
            contrast("#ffcb05", "#ffffff").unwrap() < 2.0,
            "maize on white"
        );
        assert!(
            contrast("#00274c", "#ffffff").unwrap() > 14.0,
            "navy on white"
        );
        assert_eq!(contrast("nope", "#fff"), None);
    }

    #[test]
    fn tokens_rules_and_lines_are_read() {
        let css = "/* c */\n:root { --fg: #111; --bg: #fff }\n@media (x) {\n  :root { --fg: #eee }\n}\na,\nb { color: var(--fg); border: 1px solid var(--bg) }\n";
        let tokens = root_tokens(css);
        assert_eq!(tokens.len(), 2);
        assert_eq!(tokens[0]["--fg"], "#111");
        assert_eq!(tokens[1]["--fg"], "#eee");
        let rules = rules(css);
        assert_eq!(rules.len(), 4, "two :root blocks, a and b");
        assert_eq!(rules[3].selector, "b");
        assert_eq!(rules[3].line, 6);
        assert_eq!(
            resolve(&rules[3].declarations["color"], &tokens[0]),
            Some("#111")
        );
        assert_eq!(
            colour_in(&rules[3].declarations["border"], &tokens[0]),
            Some("#fff")
        );
    }
}
