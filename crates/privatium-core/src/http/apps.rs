// Project:  Privatium™  |  File: crates/privatium-core/src/http/apps.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-03  |  Modified: 2026-09-03
// Summary:  What answers beneath an app's mount (spec/protocol.md §9.1). Tier 2: web/ served
//           as-is with index.html at the mount point, streamed in 64 KiB frames, under that
//           app's own CSP (spec/app-contract.md §5, §5.4). Tier 1: what a Lua handler
//           answered, as a response with the same headers — and, until M8, the page that
//           stands in for a view.

use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::http::header::{ALLOW, CONTENT_SECURITY_POLICY, CONTENT_TYPE, HeaderValue};
use axum::http::{Request as HttpRequest, StatusCode};
use tower::ServiceExt as _;
use tower_http::services::ServeDir;

use crate::http::{headers, shell};
use crate::lua::LuaResponse;
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

/// What a Lua handler answered, as a response (`spec/lua-api.md §3.1`). A `pv.render` is
/// answered here until M8's compiler exists: a 503 naming the view when it is on disk, a
/// 500 when it is not.
#[must_use]
pub fn lua_response(
    answer: LuaResponse,
    slug: &str,
    dir: &Path,
    csp: &str,
    solo: bool,
) -> Response {
    let mut response = match answer {
        LuaResponse::Html(body) => headers::with_body(StatusCode::OK, headers::HTML, body),
        LuaResponse::Text(body) => headers::with_body(StatusCode::OK, headers::TEXT, body),
        LuaResponse::Json(body) => headers::with_body(StatusCode::OK, headers::JSON, body),
        LuaResponse::Redirect(location) => headers::redirect(StatusCode::SEE_OTHER, &location),
        LuaResponse::NoContent => {
            let mut response = Response::new(Body::empty());
            *response.status_mut() = StatusCode::NO_CONTENT;
            response
        }
        LuaResponse::Render(view) => {
            let template = dir.join("views").join(format!("{view}.lsp"));
            if template.is_file() {
                headers::html(
                    StatusCode::SERVICE_UNAVAILABLE,
                    shell::view_not_rendered(slug, &view, solo),
                )
            } else {
                headers::html(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    shell::error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        &format!("{slug}: pv.render('{view}') names no views/{view}.lsp"),
                        solo,
                    ),
                )
            }
        }
    };
    app_headers(&mut response, csp);
    response
}

/// A failure beneath a Tier 1 mount — a Lua error, a limit, a panic — as the shell's error
/// page under the app's own headers. `detail` is the error's text; the owner reads it.
#[must_use]
pub fn lua_failure(status: StatusCode, detail: &str, csp: &str, solo: bool) -> Response {
    let mut response = headers::html(status, shell::error(status, detail, solo));
    app_headers(&mut response, csp);
    response
}

/// No route registered at `path` beneath a Tier 1 mount.
#[must_use]
pub fn not_found_under(path: &str, csp: &str, solo: bool) -> Response {
    let mut response = headers::html(StatusCode::NOT_FOUND, shell::not_found(path, solo));
    app_headers(&mut response, csp);
    response
}

/// A route exists at the path but not for this method.
#[must_use]
pub fn method_not_allowed_under(allow: &[String], csp: &str) -> Response {
    let allow = allow.join(", ");
    let mut response = headers::text(
        StatusCode::METHOD_NOT_ALLOWED,
        format!("405 Method Not Allowed — allowed: {allow}\n"),
    );
    if let Ok(value) = HeaderValue::from_str(&allow) {
        response.headers_mut().insert(ALLOW, value);
    }
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
