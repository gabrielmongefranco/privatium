// This file is part of Privatium
// crates/privatium-core/src/pair/qr.rs
// Author(s): Gabriel Mongefranco
// Created: 2026-09-06
// Last Modified: 2026-09-06
// Summary: The QR code of the node's URL (spec/protocol.md §7.1): the module matrix from the encoder
//          crate, rendered here as inline SVG for the code page and as block characters for a
//          terminal. It encodes the URL and never the pairing code.
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

use std::fmt::Write as _;

use qrcode::{EcLevel, QrCode};

use crate::icons::escape;

/// The quiet zone around the symbol, in modules — what the QR standard asks for.
const QUIET: usize = 4;

/// The most bytes a URL may hold here. A version-10 symbol at medium correction holds
/// 213 bytes; a URL longer than that is not one a phone should be typing anyway.
const MAX_BYTES: usize = 213;

/// The dark modules of a symbol, row-major, with its width.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Matrix {
    /// Modules per side, without the quiet zone.
    pub width: usize,
    /// `width * width` cells, `true` where the module is dark.
    pub dark: Vec<bool>,
}

impl Matrix {
    /// Whether the module at `(x, y)` is dark; outside the symbol is light.
    #[must_use]
    pub fn is_dark(&self, x: usize, y: usize) -> bool {
        x < self.width && y < self.width && self.dark[y * self.width + x]
    }
}

/// Encode `text` at medium error correction. `None` when it does not fit, which a
/// caller reports as a URL too long to render rather than as a failure of the node.
#[must_use]
pub fn encode(text: &str) -> Option<Matrix> {
    if text.is_empty() || text.len() > MAX_BYTES {
        return None;
    }
    let code = QrCode::with_error_correction_level(text.as_bytes(), EcLevel::M).ok()?;
    let width = code.width();
    let dark = code
        .to_colors()
        .into_iter()
        .map(|color| color == qrcode::Color::Dark)
        .collect();
    Some(Matrix { width, dark })
}

/// The symbol as inline SVG: a labelled image (`role="img"`) with a white ground and
/// black modules whatever the page's colour scheme, so a camera reads it in dark mode
/// too. `label` is the accessible name; the URL itself is text beside the image, never
/// inside it. Nothing here is a `style=` attribute, so it renders under the default CSP.
#[must_use]
pub fn svg(text: &str, label: &str) -> Option<String> {
    let matrix = encode(text)?;
    let size = matrix.width + 2 * QUIET;
    let mut path = String::new();
    for y in 0..matrix.width {
        let mut x = 0;
        while x < matrix.width {
            if !matrix.is_dark(x, y) {
                x += 1;
                continue;
            }
            let start = x;
            while x < matrix.width && matrix.is_dark(x, y) {
                x += 1;
            }
            let _ = write!(
                path,
                "M{} {}h{}v1h-{}z",
                start + QUIET,
                y + QUIET,
                x - start,
                x - start
            );
        }
    }
    // `role="img"` with a `<title>` and `focusable="false"`, as `docs/icons.md` has every
    // inline SVG carry, so a screen reader names it once and no browser tabs into it.
    let label = escape(label);
    Some(format!(
        "<svg class=\"pv-qr-image\" role=\"img\" focusable=\"false\" aria-label=\"{label}\" \
         viewBox=\"0 0 {size} {size}\" shape-rendering=\"crispEdges\"><title>{label}</title>\
         <rect width=\"{size}\" height=\"{size}\" fill=\"#ffffff\"/>\
         <path d=\"{path}\" fill=\"#000000\"/></svg>"
    ))
}

/// The symbol as lines of block characters for a terminal, two module rows per line:
/// `█` for two dark, `▀` and `▄` for one, a space for none, with the quiet zone drawn
/// as light. Dark modules are the ink, so the code reads on a light terminal directly
/// and on a dark one as the inverted symbol most phone cameras also accept.
#[must_use]
pub fn text(text: &str) -> Option<String> {
    let matrix = encode(text)?;
    let size = matrix.width + 2 * QUIET;
    let dark =
        |x: usize, y: usize| x >= QUIET && y >= QUIET && matrix.is_dark(x - QUIET, y - QUIET);
    let mut out = String::new();
    let mut y = 0;
    while y < size {
        for x in 0..size {
            let (top, bottom) = (dark(x, y), y + 1 < size && dark(x, y + 1));
            out.push(match (top, bottom) {
                (true, true) => '█',
                (true, false) => '▀',
                (false, true) => '▄',
                (false, false) => ' ',
            });
        }
        out.push('\n');
        y += 2;
    }
    Some(out)
}

// AGENTS.md, Style: unwrap() is permitted in tests. The crate-level deny reaches unit
// tests inside src/, so each one opts out where it is declared.
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spec_7_1_qr_encodes_the_url_and_renders_both_ways() {
        let url = "http://192.0.2.10:8420";
        let matrix = encode(url).unwrap();
        assert_eq!(matrix.dark.len(), matrix.width * matrix.width);
        // The three finder patterns are dark at their corners.
        assert!(matrix.is_dark(0, 0));
        assert!(matrix.is_dark(matrix.width - 1, 0));
        assert!(matrix.is_dark(0, matrix.width - 1));
        assert!(!matrix.is_dark(matrix.width, 0), "outside is light");

        let svg = svg(url, "QR code for \"the\" URL").unwrap();
        assert!(svg.starts_with("<svg "));
        assert!(svg.contains("role=\"img\""));
        assert!(svg.contains("aria-label=\"QR code for &quot;the&quot; URL\""));
        assert!(svg.contains("<title>QR code for &quot;the&quot; URL</title>"));
        assert!(svg.contains("focusable=\"false\""));
        assert!(!svg.contains("style="));
        assert!(
            !svg.contains(url),
            "the URL is text beside the image, not inside it"
        );

        let rendered = text(url).unwrap();
        let lines: Vec<&str> = rendered.lines().collect();
        let size = matrix.width + 2 * QUIET;
        assert_eq!(lines.len(), size.div_ceil(2));
        assert!(lines.iter().all(|line| line.chars().count() == size));
        assert!(lines.iter().any(|line| line.contains('█')));
    }

    #[test]
    fn empty_and_oversized_input_render_nothing() {
        assert!(encode("").is_none());
        assert!(encode(&"x".repeat(MAX_BYTES + 1)).is_none());
        assert!(encode(&"x".repeat(MAX_BYTES)).is_some());
        assert!(svg("", "x").is_none());
        assert!(text("").is_none());
    }
}
