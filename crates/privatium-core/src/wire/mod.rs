// Project:  Privatium™  |  File: crates/privatium-core/src/wire/mod.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-03  |  Modified: 2026-09-03
// Summary:  core::handle(Request) -> Response (ADR 0003): the one entry point for application
//           traffic. Bodies are streams in both directions. The router is built from
//           Node::mounts(); the auth layer runs here so every adapter gets it; the §9.3
//           headers go on every response on the way out. How the Node is shared is decided
//           here too — one mutex, taken for the synchronous part of a request and released
//           before anything awaits.

use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use axum::body::to_bytes;
use axum::http::header::{CONTENT_LENGTH, HOST};
use axum::http::{Method, StatusCode};
use tower::{Layer as _, ServiceExt as _};

use crate::app::{LoadReport, Tier};
use crate::config::Mode;
use crate::http::shell::{Context, Notice, NoticeKind};
use crate::http::{self, AuthLayer, Csrf, api, apps, assets, headers, shell, skills};
use crate::{Error, Node};

pub mod router;

pub use router::{FRAMEWORK_PREFIXES, Route, Router, SettingsPage, url};

/// A request or response body: a boxed stream of byte frames, never a `Vec<u8>`
/// (`AGENTS.md`). This is `axum`'s body type, taken for its shape rather than for axum — a
/// wrapper over any `http_body::Body` — so an adapter with a stream of its own wraps it and
/// converts nothing.
pub type Body = axum::body::Body;

/// A request, as the `http` crate spells one, with a streaming body.
pub type Request = axum::http::Request<Body>;

/// A response, likewise.
pub type Response = axum::http::Response<Body>;

pub use crate::http::auth::{Device, Peer};

/// The node behind `handle`, and everything a request needs that the node itself does not
/// hold: the load report, the CSRF issuer, the auth layer, the origin to fall back on.
///
/// **How the node is shared.** `Node` is `Send` and not `Sync` — its DuckDB connections and
/// its log writers are single-threaded things — so it lives behind one `Mutex`. A request
/// takes the lock for its synchronous part: resolving the route, the stat-based
/// `refresh_app`/`refresh`, the `_sys` reads and the HTML they render, and releases it
/// before anything is awaited, so file streaming for a Tier 2 app never holds it. Every
/// `app_conn()` is taken and dropped inside one locked section, which is what keeps M5's
/// rule — never hold one across a privileged window — true without further machinery. An
/// actor would give the same serialization with a channel in between; the mutex is the
/// same guarantee with less to read, and `refresh_app`/`load_seed` need `&mut Node` anyway.
/// A settings page holds the lock for the milliseconds its reads take. When the data API
/// arrives (M9) and app SQL runs per request, the thing to revisit is a read-write split per
/// app, not the mutex.
pub struct Handler {
    node: Arc<Mutex<Node>>,
    report: LoadReport,
    csrf: Csrf,
    auth: AuthLayer,
    mode: Mode,
    /// `http://127.0.0.1:<port>` — what `App::csp().header_for` is rendered against when a
    /// request carries no usable `Host`, which an in-process adapter's may not.
    default_origin: String,
}

impl std::fmt::Debug for Handler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Handler")
            .field("mode", &self.mode)
            .field("default_origin", &self.default_origin)
            .finish_non_exhaustive()
    }
}

impl Handler {
    /// Wrap a node whose apps are loaded, with what `load_apps` reported.
    #[must_use]
    pub fn new(node: Node, report: LoadReport) -> Self {
        let csrf = Csrf::new(node.identity());
        let auth = node.auth_layer();
        let mode = node.config().node.mode;
        let default_origin = format!("http://{}:{}", Ipv4Addr::LOCALHOST, node.config().node.port);
        Self {
            node: Arc::new(Mutex::new(node)),
            report,
            csrf,
            auth,
            mode,
            default_origin,
        }
    }

    /// The shared node. Lock it briefly; never across an `await`.
    #[must_use]
    pub fn node(&self) -> &Arc<Mutex<Node>> {
        &self.node
    }

    /// What `load_apps` reported when this handler was built.
    #[must_use]
    pub fn report(&self) -> &LoadReport {
        &self.report
    }

    /// The CSRF issuer — for a test that needs a valid token, or a template helper.
    #[must_use]
    pub fn csrf(&self) -> &Csrf {
        &self.csrf
    }

    /// The origin `App::csp().header_for` is rendered against for `request`: its `Host`
    /// header when it names an authority, otherwise this node's loopback origin.
    ///
    /// The value goes into a header verbatim, so it is accepted only when it is a bare
    /// authority — no `;`, no spaces — and `http` is the only scheme Phase 1 speaks.
    #[must_use]
    pub fn origin_of(&self, request: &Request) -> String {
        request
            .headers()
            .get(HOST)
            .and_then(|host| host.to_str().ok())
            .filter(|host| is_authority(host))
            .map_or_else(
                || self.default_origin.clone(),
                |host| format!("http://{host}"),
            )
    }

    /// `core::handle` (ADR 0003). Infallible: anything that goes wrong is a response.
    pub async fn handle(&self, request: Request) -> Response {
        let inner = tower::service_fn(|request: Request| async move {
            Ok::<Response, std::convert::Infallible>(self.dispatch(request).await)
        });
        let mut response = match self.auth.layer(inner).oneshot(request).await {
            Ok(response) => response,
            Err(never) => match never {},
        };
        headers::secure(&mut response, headers::CSP_DEFAULT);
        response
    }

    fn lock(&self) -> MutexGuard<'_, Node> {
        self.node.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn solo(&self) -> bool {
        self.mode == Mode::Solo
    }

    async fn dispatch(&self, request: Request) -> Response {
        let method = request.method().clone();
        let path = request.uri().path().to_owned();
        let head = method == Method::HEAD;
        let get = method == Method::GET || head;

        let route = {
            let node = self.lock();
            Router::new(
                self.mode,
                node.mounts().map(|(mount, app)| (mount, app.slug())),
            )
            .resolve(&path)
        };

        let response = match route {
            Route::Launcher => {
                if !get {
                    return headers::method_not_allowed("GET, HEAD");
                }
                self.render(shell::launcher)
            }
            Route::Settings(page) => {
                if !get {
                    return headers::method_not_allowed("GET, HEAD");
                }
                if page == SettingsPage::Apps {
                    self.refresh_all_apps();
                }
                self.render(|cx| shell::settings(cx, page, None))
            }
            Route::Seed { slug } => {
                if method != Method::POST {
                    return headers::method_not_allowed("POST");
                }
                return self.seed(&slug, &path, request).await;
            }
            Route::Health => {
                if !get {
                    return headers::method_not_allowed("GET, HEAD");
                }
                let node = self.lock();
                headers::with_body(StatusCode::OK, headers::JSON, api::health(&node))
            }
            Route::Manifest => {
                if !get {
                    return headers::method_not_allowed("GET, HEAD");
                }
                let mut node = self.lock();
                match node.refresh().and_then(|_| api::manifest(&node)) {
                    Ok(value) => headers::json(StatusCode::OK, &value),
                    Err(error) => self.failure(&error),
                }
            }
            Route::Skill { name } => {
                if !get {
                    return headers::method_not_allowed("GET, HEAD");
                }
                match skills::skill(&name) {
                    Some(text) => {
                        let mut response =
                            headers::with_body(StatusCode::OK, headers::MARKDOWN, text);
                        headers::revalidate(&mut response);
                        response
                    }
                    None => self.not_found(&path),
                }
            }
            Route::SkillBundle => {
                if !get {
                    return headers::method_not_allowed("GET, HEAD");
                }
                let mut response =
                    headers::with_body(StatusCode::OK, headers::ZIP, skills::bundle());
                headers::revalidate(&mut response);
                response
            }
            Route::Static { rest } => {
                if !get {
                    return headers::method_not_allowed("GET, HEAD");
                }
                match assets::get(&rest) {
                    Some(asset) => {
                        let mut response =
                            headers::with_body(StatusCode::OK, asset.content_type, asset.bytes);
                        headers::revalidate(&mut response);
                        response
                    }
                    None => self.not_found(&path),
                }
            }
            Route::Redirect { to } => headers::redirect(StatusCode::PERMANENT_REDIRECT, &to),
            Route::NotFound => self.not_found(&path),
            Route::App { slug, mount, rest } => {
                return self.app(&slug, &mount, &rest, request).await;
            }
        };

        if head {
            headers::strip_body(response)
        } else {
            response
        }
    }

    /// A route beneath an app's mount. The read path refreshes the app first — the
    /// `echo >>` reload of `apps/hello/README.md` — then serves it by tier.
    async fn app(&self, slug: &str, mount: &str, rest: &str, request: Request) -> Response {
        let origin = self.origin_of(&request);
        let plan = {
            let mut node = self.lock();
            if let Err(error) = node.refresh_app(slug) {
                return self.failure(&error);
            }
            let Some(app) = node.app(slug) else {
                return self.not_found(request.uri().path());
            };
            let csp = app.csp().header_for(&origin);
            match app.manifest().app.tier {
                Tier::Web => Ok((app.dir().join("web"), csp)),
                Tier::Lua => Err(apps::no_handler(slug, &csp, self.solo())),
                // A tier 3 entry is never mounted (`App::mount`), so the router never gets
                // here; if it ever did, nothing is served.
                Tier::Rust => return self.not_found(request.uri().path()),
            }
        };
        match plan {
            Ok((web_dir, csp)) => {
                apps::serve_web(web_dir, mount, rest, request, &csp, self.solo()).await
            }
            Err(response) => response,
        }
    }

    /// `POST /settings/apps/<slug>/seed`: the owner's explicit act
    /// (`spec/app-contract.md §9`), behind `csrf()`.
    async fn seed(&self, slug: &str, path: &str, request: Request) -> Response {
        // A declared length past the limit is refused before the body is read at all, so a
        // client that sent `Expect: 100-continue` never sends it. A body with no declared
        // length is read up to the limit and refused there; the rest is never read.
        let declared = request
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<usize>().ok());
        if declared.is_some_and(|length| length > http::FORM_LIMIT) {
            return headers::text(
                StatusCode::PAYLOAD_TOO_LARGE,
                "413 Payload Too Large: a form here carries a token and nothing else
",
            );
        }
        let body = match to_bytes(request.into_body(), http::FORM_LIMIT).await {
            Ok(body) => body,
            Err(_) => {
                return headers::text(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "413 Payload Too Large: a form here carries a token and nothing else\n",
                );
            }
        };
        let form = http::parse_form(&body);
        let token = form
            .get(http::csrf::FIELD)
            .map(String::as_str)
            .unwrap_or_default();
        if !self.csrf.verify(path, token) {
            return headers::text(
                StatusCode::FORBIDDEN,
                "403 Forbidden: the form token is missing, stale, or for another form — reload \
                 the page and try again\n",
            );
        }

        let outcome = self.lock().load_seed(slug);
        match outcome {
            Ok(seeded) => {
                let mut response = headers::redirect(
                    StatusCode::SEE_OTHER,
                    &format!("{}#app-{slug}", SettingsPage::Apps.path()),
                );
                // htmx follows `HX-Redirect` client-side; a plain form follows `Location`.
                if let Ok(value) = axum::http::HeaderValue::from_str(SettingsPage::Apps.path()) {
                    response.headers_mut().insert("hx-redirect", value);
                }
                let _ = seeded;
                response
            }
            Err(error) => {
                let status = match error {
                    Error::SeedRefused { .. } => StatusCode::CONFLICT,
                    Error::AppNotLoaded { .. } | Error::NoSeed { .. } => StatusCode::NOT_FOUND,
                    _ => StatusCode::INTERNAL_SERVER_ERROR,
                };
                let notice = Notice {
                    kind: NoticeKind::Warn,
                    text: error.to_string(),
                };
                let page = {
                    let node = self.lock();
                    let cx = Context {
                        node: &node,
                        report: &self.report,
                        csrf: &self.csrf,
                    };
                    shell::settings(&cx, SettingsPage::Apps, Some(&notice))
                };
                match page {
                    Ok(html) => headers::html(status, html),
                    Err(error) => self.failure(&error),
                }
            }
        }
    }

    /// Render a shell page under the lock, after refreshing `_sys` (per request: the read
    /// path notices a `_sys` log that grew behind it).
    fn render(&self, page: impl FnOnce(&Context<'_>) -> crate::Result<String>) -> Response {
        let mut node = self.lock();
        if let Err(error) = node.refresh() {
            return self.failure(&error);
        }
        let cx = Context {
            node: &node,
            report: &self.report,
            csrf: &self.csrf,
        };
        match page(&cx) {
            Ok(html) => headers::html(StatusCode::OK, html),
            Err(error) => self.failure(&error),
        }
    }

    /// The apps page shows each app's tables, so each loaded app is refreshed first. A
    /// refresh that fails is not fatal to the page; the app's row will say what it can.
    fn refresh_all_apps(&self) {
        let mut node = self.lock();
        let slugs: Vec<String> = node.apps().map(|app| app.slug().to_owned()).collect();
        for slug in slugs {
            let _ = node.refresh_app(&slug);
        }
    }

    fn not_found(&self, path: &str) -> Response {
        headers::html(StatusCode::NOT_FOUND, shell::not_found(path, self.solo()))
    }

    fn failure(&self, error: &Error) -> Response {
        let status = match error {
            Error::AppNotLoaded { .. } => StatusCode::NOT_FOUND,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        headers::html(
            status,
            shell::error(status, &error.to_string(), self.solo()),
        )
    }
}

/// `host[:port]` and nothing else: letters, digits, `.`, `-`, `:`, and brackets for IPv6.
/// What may follow `http://` in a CSP `host-source`.
fn is_authority(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 255
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':' | b'[' | b']'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorities() {
        assert!(is_authority("127.0.0.1:8420"));
        assert!(is_authority("localhost"));
        assert!(is_authority("[::1]:8420"));
        assert!(!is_authority("a; script-src *"));
        assert!(!is_authority("host/path"));
        assert!(!is_authority(""));
    }

    /// The whole sharing story rests on this: a `Mutex<Node>` is `Sync` only if `Node` is
    /// `Send`.
    #[test]
    fn the_node_can_be_shared_across_threads() {
        fn assert_send<T: Send>() {}
        fn assert_sync<T: Sync>() {}
        assert_send::<Node>();
        assert_sync::<Handler>();
    }
}
