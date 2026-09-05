// Project:  Privatium™  |  File: crates/privatium-core/src/http/pairing.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-05  |  Modified: 2026-09-05
// Summary:  Data-free browser bootstrap and plain-HTTP disclosure (§7.7, §8.4).

use super::{assets, headers};
use crate::wire::Response;
use axum::http::{StatusCode, Uri};

/// The disclosure of spec/protocol.md §7.7, for every plain-HTTP visit.
pub const DISCLOSURE: &str = "On every visit over plain HTTP, someone who can change network traffic can replace this client and read your data and stored device keys. Encryption protects against listening, but cannot verify the downloaded client.";

/// Render a bootstrap with the requested path and public node ID only. Query strings
/// are attribute-escaped; neither application content nor a CSRF token is read.
pub fn bootstrap(uri: &Uri, node: &str) -> Response {
    let escape = crate::icons::escape;
    let path = uri.path_and_query().map_or("/", |p| p.as_str());
    headers::html(
        StatusCode::OK,
        format!(
            r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Connect — Privatium</title>
<link rel="stylesheet" href="/static/shell.css" integrity="{}">
<script type="module" src="/static/client.js" integrity="{}"></script>
</head><body data-pv-bootstrap data-path="{}" data-node="{}">
<main id="main" tabindex="-1"><h1>Connect to Privatium</h1>
<p role="status">Connecting to your node.</p><p>{}</p>
<noscript>Pairing from another device needs JavaScript. You can use Privatium without JavaScript in a browser on the node itself.</noscript>
</main></body></html>"#,
            assets::integrity("shell.css"),
            assets::integrity("client.js"),
            escape(path),
            escape(node),
            DISCLOSURE
        ),
    )
}
