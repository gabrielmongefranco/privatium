// Project:  Privatium™  |  File: crates/privatium-core/src/http/pairing.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-05  |  Modified: 2026-09-06
// Summary:  The data-free browser bootstrap (§8.4) with the pairing screen inside it
//           (§7.2, §7.7): the sixteen-glyph pad with a label beneath every glyph, the
//           word field beside it, and the status region the three outcomes are said in.
//           The markup is rendered here so the PV4xx checks hold it; client.js shows it
//           when the browser holds no pairing and wires it, and a <noscript> browser
//           never reaches it.

use std::fmt::Write as _;

use super::{assets, headers};
use crate::icons::escape;
use crate::pair::GLYPHS;
use crate::wire::Response;
use axum::http::{StatusCode, Uri};

/// The disclosure of spec/protocol.md §7.7, for every plain-HTTP visit.
pub const DISCLOSURE: &str = "On every visit over plain HTTP, someone who can change network traffic can replace this client and read your data and stored device keys. Encryption protects against listening, but cannot verify the downloaded client.";

/// Render a bootstrap with the requested path, the public node ID and its display name
/// only. Query strings are attribute-escaped; neither application content nor a CSRF
/// token is read.
pub fn bootstrap(uri: &Uri, node: &str, name: &str) -> Response {
    let path = uri.path_and_query().map_or("/", |p| p.as_str());
    let mut html = format!(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Connect — Privatium</title>
<link rel="stylesheet" href="/static/shell.css" integrity="{}">
<script type="module" src="/static/client.js" integrity="{}"></script>
</head><body data-pv-bootstrap data-path="{}" data-node="{}" data-name="{}">
<main id="main" tabindex="-1"><h1>Connect to Privatium</h1>
<p id="pv-connecting" role="status">Connecting to your space.</p>
<noscript><p>Pairing from another device needs JavaScript. You can use Privatium without JavaScript in a browser on the space itself.</p></noscript>
"#,
        assets::integrity("shell.css"),
        assets::integrity("client.js"),
        escape(path),
        escape(node),
        escape(name)
    );
    pairing_screen(&mut html, node, name);
    let _ = write!(
        html,
        "<p class=\"pv-muted pv-disclosure\">{DISCLOSURE}</p>\n</main></body></html>"
    );
    headers::html(StatusCode::OK, html)
}

/// The pairing screen (`spec/protocol.md §7.2`), hidden until the client finds no
/// pairing in storage. Completable without reading — four taps on the pad, then Pair —
/// and without seeing: every control is labelled, the glyph inside each key is hidden
/// from a screen reader so its label is read once, and the word field takes the two
/// words `§7.2` renders the same code as.
fn pairing_screen(out: &mut String, node: &str, name: &str) {
    let _ = write!(
        out,
        "<section id=\"pv-pair\" hidden aria-labelledby=\"pv-pair-title\">\n\
         <h2 id=\"pv-pair-title\">Pair this device with {} <code>{}</code></h2>\n\
         <p>Read the pairing code from the space's screen. Tap its four emoji here, in order, or \
         type its two words.</p>\n\
         <div class=\"pv-pad\" role=\"group\" aria-label=\"Emoji pad\">\n",
        escape(name),
        escape(node)
    );
    for (index, glyph) in GLYPHS.iter().enumerate() {
        let _ = writeln!(
            out,
            "<button type=\"button\" class=\"pv-pad-key\" data-glyph=\"{index}\">\
             <span class=\"pv-glyph\" aria-hidden=\"true\">{}</span><span class=\"pv-glyph-label\">{}</span></button>",
            glyph.glyph, glyph.label
        );
    }
    out.push_str(
        "</div>\n\
         <p class=\"pv-chosen\">Chosen: <output id=\"pv-chosen\" aria-live=\"polite\">none yet</output> \
         <button type=\"button\" id=\"pv-undo\" class=\"pv-btn\">Remove last</button> \
         <button type=\"button\" id=\"pv-clear\" class=\"pv-btn\">Clear</button></p>\n\
         <form id=\"pv-pair-form\" method=\"post\" action=\"#\">\n\
         <label for=\"pv-words\">Or type the two words</label>\n\
         <input id=\"pv-words\" name=\"words\" autocomplete=\"off\" autocapitalize=\"none\" \
         spellcheck=\"false\" placeholder=\"amber otter\">\n\
         <label for=\"pv-label\">Name this device (optional)</label>\n\
         <input id=\"pv-label\" name=\"label\" maxlength=\"80\" autocomplete=\"off\">\n\
         <button type=\"submit\" id=\"pv-pair-submit\" class=\"pv-btn pv-btn-primary\">Pair</button>\n\
         </form>\n\
         <p id=\"pv-pair-status\" role=\"status\"></p>\n\
         <p class=\"pv-muted\">Pairing has to be open on the space: its Settings › Devices › \
         Open pairing, or <code>privatium pair</code> in a terminal there.</p>\n\
         </section>\n",
    );
}
