// Project:  Privatium™  |  File: crates/privatium-core/src/http/apps.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-03  |  Modified: 2026-09-03
// Summary:  What answers beneath an app's mount (spec/protocol.md §9.1). Tier 2: web/ served
//           as-is with index.html at the mount point, streamed in 64 KiB frames, under that
//           app's own CSP (spec/app-contract.md §5, §5.4). Tier 1: the mount exists and says
//           plainly that this build has no handler, until M7.

use std::path::PathBuf;

use axum::body::Body;
use axum::http::header::{CONTENT_SECURITY_POLICY, CONTENT_TYPE, HeaderValue};
use axum::http::{Request as HttpRequest, StatusCode};
use tower::ServiceExt as _;
use tower_http::services::ServeDir;

use crate::http::{headers, shell};
use crate::wire::{Request, Response};

/// Serve `rest` — the path beneath the mount, `/` for the mount point — out of `web_dir`.
///
/// `ServeDir` does the file work: it percent-decodes, refuses any `..` component, appends
/// `index.html` to a directory, guesses the content type, honours `Range` and
/// `If-Modified-Since`, and reads the file as a stream. The sub-request it sees carries the
/// remainder path only; `redirect_path_prefix` puts the mount back on the one redirect it
/// may issue (a directory without its trailing slash), so no adapter ever rewrites a path.
pub async fn serve_web(
    web_dir: PathBuf,
    mount: &str,
    rest: &str,
    request: Request,
    csp: &str,
    solo: bool,
) -> Response {
    let (parts, body) = request.into_parts();
    let mut uri = rest.to_owned();
    if let Some(query) = parts.uri.query() {
        uri.push('?');
        uri.push_str(query);
    }
    let mut sub = match HttpRequest::builder()
        .method(parts.method)
        .uri(uri)
        .body(body)
    {
        Ok(sub) => sub,
        Err(error) => {
            return headers::text(
                StatusCode::BAD_REQUEST,
                format!("400 Bad Request: {error}\n"),
            );
        }
    };
    *sub.headers_mut() = parts.headers;

    let service = ServeDir::new(web_dir)
        .append_index_html_on_directories(true)
        .redirect_path_prefix(mount.trim_end_matches('/'));
    let mut response = match service.oneshot(sub).await {
        Ok(response) => response.map(Body::new),
        Err(never) => match never {},
    };

    if response.status() == StatusCode::NOT_FOUND {
        response = headers::html(
            StatusCode::NOT_FOUND,
            shell::not_found(&url_path(mount, rest), solo),
        );
    }
    app_headers(&mut response, csp);
    response
}

/// The answer for a Tier 1 route in a build without the Lua host.
#[must_use]
pub fn no_handler(slug: &str, csp: &str, solo: bool) -> Response {
    let mut response = headers::html(
        StatusCode::SERVICE_UNAVAILABLE,
        shell::no_handler(slug, solo),
    );
    app_headers(&mut response, csp);
    response
}

/// The app's own policy — `App::csp().header_for(origin)`, never `header()` — and
/// `no-store`, on every response carrying the app's bytes (`§9.3`).
fn app_headers(response: &mut Response, csp: &str) {
    if let Ok(value) = HeaderValue::from_str(csp) {
        response
            .headers_mut()
            .insert(CONTENT_SECURITY_POLICY, value);
    }
    if !response.headers().contains_key(CONTENT_TYPE) {
        response.headers_mut().insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/octet-stream"),
        );
    }
    headers::secure(response, csp);
}

fn url_path(mount: &str, rest: &str) -> String {
    crate::wire::router::url(mount, rest)
}
