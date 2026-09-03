// Project:  Privatium™  |  File: crates/privatium-core/src/http/headers.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-03  |  Modified: 2026-09-03
// Summary:  The headers of spec/protocol.md §9.3 and the small set of response shapes the
//           shell and the API answer with. Every response leaving core::handle passes
//           through `secure`, so a 403 from the auth layer and a 500 from a failed page
//           carry the same policy a page does.

use axum::body::Body;
use axum::http::header::{
    ALLOW, CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, HeaderValue, LOCATION,
    REFERRER_POLICY, X_CONTENT_TYPE_OPTIONS,
};
use axum::http::{Response, StatusCode};

/// `spec/protocol.md §9.3`, verbatim: the framework's own policy and the floor every app's
/// starts from (`app::Csp`).
pub const CSP_DEFAULT: &str = "default-src 'self'; script-src 'self'; object-src 'none'; \
                               base-uri 'none'; form-action 'self'; frame-ancestors 'none'";

/// `Cache-Control` for the embedded assets under `/static/*` and the skill documents —
/// the responses `§9.3` exempts because they carry no data. Revalidated, never stale.
const CACHE_REVALIDATE: &str = "no-cache";

/// `Cache-Control: no-store` — `§9.3`, on every response containing app data.
const NO_STORE: &str = "no-store";

pub const HTML: &str = "text/html; charset=utf-8";
pub const JSON: &str = "application/json";
pub const TEXT: &str = "text/plain; charset=utf-8";
pub const MARKDOWN: &str = "text/markdown; charset=utf-8";
pub const ZIP: &str = "application/zip";

/// Apply `§9.3` to a response: the CSP given if none is set yet, `nosniff`, and
/// `no-referrer`. Idempotent, so a response built with an app's policy keeps it when the
/// handler applies the default on the way out.
pub fn secure(response: &mut Response<Body>, csp: &str) {
    let headers = response.headers_mut();
    if !headers.contains_key(CONTENT_SECURITY_POLICY)
        && let Ok(value) = HeaderValue::from_str(csp)
    {
        headers.insert(CONTENT_SECURITY_POLICY, value);
    }
    headers.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    if !headers.contains_key(CACHE_CONTROL) {
        headers.insert(CACHE_CONTROL, HeaderValue::from_static(NO_STORE));
    }
}

/// Mark a response as one of the two cacheable kinds.
pub fn revalidate(response: &mut Response<Body>) {
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static(CACHE_REVALIDATE));
}

/// A response with a body and a content type. `Cache-Control` is left for `secure`.
pub fn with_body(status: StatusCode, content_type: &str, body: impl Into<Body>) -> Response<Body> {
    let mut response = Response::new(body.into());
    *response.status_mut() = status;
    if let Ok(value) = HeaderValue::from_str(content_type) {
        response.headers_mut().insert(CONTENT_TYPE, value);
    }
    response
}

/// An HTML page.
pub fn html(status: StatusCode, page: String) -> Response<Body> {
    with_body(status, HTML, page)
}

/// A JSON document, serialized here so a serialization failure is a 500 and not a panic.
pub fn json(status: StatusCode, value: &serde_json::Value) -> Response<Body> {
    match serde_json::to_string(value) {
        Ok(text) => with_body(status, JSON, text),
        Err(error) => with_body(
            StatusCode::INTERNAL_SERVER_ERROR,
            TEXT,
            format!("could not serialize the response: {error}"),
        ),
    }
}

/// Plain text.
pub fn text(status: StatusCode, body: impl Into<String>) -> Response<Body> {
    with_body(status, TEXT, body.into())
}

/// A redirect. `303 See Other` after a POST, `308 Permanent Redirect` for a missing
/// trailing slash — both keep the method semantics honest.
pub fn redirect(status: StatusCode, location: &str) -> Response<Body> {
    let mut response = with_body(status, TEXT, format!("see {location}\n"));
    if let Ok(value) = HeaderValue::from_str(location) {
        response.headers_mut().insert(LOCATION, value);
    }
    response
}

/// `405`, naming what would have worked.
pub fn method_not_allowed(allow: &'static str) -> Response<Body> {
    let mut response = text(
        StatusCode::METHOD_NOT_ALLOWED,
        format!("405 Method Not Allowed — allowed: {allow}\n"),
    );
    response
        .headers_mut()
        .insert(ALLOW, HeaderValue::from_static(allow));
    response
}

/// Strip the body for a `HEAD` request, keeping the headers a `GET` would have sent.
pub fn strip_body(response: Response<Body>) -> Response<Body> {
    let (parts, _) = response.into_parts();
    Response::from_parts(parts, Body::empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secure_is_idempotent_and_keeps_an_existing_policy() {
        let mut response = text(StatusCode::OK, "x");
        response.headers_mut().insert(
            CONTENT_SECURITY_POLICY,
            HeaderValue::from_static("default-src 'none'"),
        );
        secure(&mut response, CSP_DEFAULT);
        secure(&mut response, CSP_DEFAULT);
        let headers = response.headers();
        assert_eq!(headers[CONTENT_SECURITY_POLICY], "default-src 'none'");
        assert_eq!(headers[X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert_eq!(headers[REFERRER_POLICY], "no-referrer");
        assert_eq!(headers[CACHE_CONTROL], "no-store");
        assert_eq!(headers.get_all(REFERRER_POLICY).iter().count(), 1);
    }

    #[test]
    fn the_default_policy_is_section_9_3_verbatim() {
        assert_eq!(
            CSP_DEFAULT,
            "default-src 'self'; script-src 'self'; object-src 'none'; base-uri 'none'; \
             form-action 'self'; frame-ancestors 'none'"
        );
    }
}
