// Project:  Privatium™  |  File: crates/privatium-core/tests/wire.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-03  |  Modified: 2026-09-03
// Summary:  core::handle against spec/protocol.md §9 and ADR 0003 — every route reachable
//           with no listener, the headers of §9.3 on every response, nothing leaked
//           unauthenticated (§9.2), solo mode at `/` with the framework prefixes winning
//           (§9.1), Tier 2 served under its own CSP (spec/app-contract.md §5.4), the seed
//           behind a POST (§9), and bodies that stream. Tier 1 routes are tests/lua.rs.

// AGENTS.md, Style: unwrap() is permitted in tests, and a test that hides a failure
// behind `?` is worse than one that panics with a line number.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::collections::BTreeSet;
use std::fs;
use std::net::SocketAddr;
use std::pin::Pin;

use axum::body::{HttpBody as _, to_bytes};
use axum::http::header::{
    CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, HOST, LOCATION, REFERRER_POLICY,
    X_CONTENT_TYPE_OPTIONS,
};
use axum::http::{Method, StatusCode};
use common::{
    APP, event, hand_append, lua_manifest, repo_apps_dir, ts_offset_secs, write_app, write_web_app,
};
use privatium_core::app::Warning;
use privatium_core::{AppRoot, Body, Handler, LoadReport, Node, Peer, Request, Response};

/// `spec/protocol.md §9.3`, verbatim.
const DEFAULT_CSP: &str = "default-src 'self'; script-src 'self'; object-src 'none'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'";

/// A node with the three reference apps loaded as bundled, plus whatever `local` holds.
fn open(root: &tempfile::TempDir) -> (Node, LoadReport) {
    let mut node = Node::open(root.path()).unwrap();
    let roots = [
        AppRoot::local(node.paths().apps_dir()),
        AppRoot::bundled(repo_apps_dir()),
    ];
    let report = node.load_apps(&roots).unwrap();
    (node, report)
}

fn handler(root: &tempfile::TempDir) -> Handler {
    let (node, report) = open(root);
    Handler::new(node, report)
}

/// A solo node for `app`, with the reference apps available to it.
fn solo(root: &tempfile::TempDir, app: &str) -> Handler {
    fs::write(
        root.path().join("config.toml"),
        format!("[node]\nmode = \"solo\"\napp = \"{app}\"\n"),
    )
    .unwrap();
    handler(root)
}

fn request(method: Method, path: &str) -> Request {
    axum::http::Request::builder()
        .method(method)
        .uri(path)
        .body(Body::empty())
        .unwrap()
}

fn get(path: &str) -> Request {
    request(Method::GET, path)
}

fn with_host(mut request: Request, host: &str) -> Request {
    request.headers_mut().insert(HOST, host.parse().unwrap());
    request
}

fn with_peer(mut request: Request, addr: &str) -> Request {
    let addr: SocketAddr = addr.parse().unwrap();
    request.extensions_mut().insert(Peer(addr));
    request
}

async fn body_of(response: Response) -> String {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn header<'a>(response: &'a Response, name: &axum::http::HeaderName) -> &'a str {
    response
        .headers()
        .get(name)
        .map(|v| v.to_str().unwrap())
        .unwrap_or_default()
}

/// The paths every test reaches for, with the status a fresh host-mode node answers.
const HOST_ROUTES: &[(&str, u16)] = &[
    ("/", 200),
    ("/settings", 200),
    ("/settings/apps", 200),
    ("/settings/data", 200),
    ("/settings/devices", 200),
    ("/api/v1/health", 200),
    ("/api/v1/manifest", 200),
    ("/skills/privatium-overview.md", 200),
    ("/skills/bundle.zip", 200),
    ("/static/shell.css", 200),
    ("/static/htmx.min.js", 200),
    ("/a/sketch/", 200),
    ("/a/sketch/style.css", 200),
    ("/a/hello/", 200),
    ("/a/animals/static/animals.css", 200),
    ("/a/animals/play", 404),
    ("/a/sketch", 308),
    ("/a/nope/", 404),
    ("/a/", 404),
    ("/nope", 404),
    ("/api/nope", 404),
    ("/skills/nope.md", 404),
    ("/static/pv.js", 200),
    ("/a/sketch/api/node", 200),
    ("/a/hello/api/schema", 200),
    ("/a/sketch/api/nope", 404),
    ("/settings/nope", 404),
];

// ---------------------------------------------------------------------------------------
// §9.1 — every prefix, through handle, with no socket
// ---------------------------------------------------------------------------------------

/// `spec/protocol.md §9.1`, ADR 0003 — every namespace answers through `handle`, an
/// unknown slug is a 404 and not a panic, and a Tier 1 view renders inside the
/// framework's page frame (`spec/lua-api.md §4.1`).
#[tokio::test]
async fn test_spec_9_1_every_prefix_reachable_through_handle() {
    let root = tempfile::tempdir().unwrap();
    let handler = handler(&root);
    for (path, status) in HOST_ROUTES {
        let response = handler.handle(get(path)).await;
        assert_eq!(response.status().as_u16(), *status, "{path}");
    }
    let redirect = handler.handle(get("/a/sketch")).await;
    assert_eq!(header(&redirect, &LOCATION), "/a/sketch/");
    let tier1 = handler.handle(get("/a/hello/")).await;
    let text = body_of(tier1).await;
    assert!(text.starts_with("<!doctype html>"), "{text}");
    assert!(text.contains("<title>Hello — Privatium</title>"), "{text}");
    assert!(text.contains("We haven't met yet."), "{text}");
    assert!(text.contains("href=\"/a/hello/edit\""), "{text}");

    // HEAD answers like GET without a body; the wrong method says which would work.
    let head = handler.handle(request(Method::HEAD, "/settings")).await;
    assert_eq!(head.status(), StatusCode::OK);
    assert_eq!(header(&head, &CONTENT_TYPE), "text/html; charset=utf-8");
    assert!(body_of(head).await.is_empty());
    let post = handler.handle(request(Method::POST, "/")).await;
    assert_eq!(post.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(header(&post, &axum::http::header::ALLOW), "GET, HEAD");
    let delete = handler
        .handle(request(Method::DELETE, "/api/v1/health"))
        .await;
    assert_eq!(delete.status(), StatusCode::METHOD_NOT_ALLOWED);
}

// ---------------------------------------------------------------------------------------
// §9.2 — unauthenticated endpoints leak nothing
// ---------------------------------------------------------------------------------------

/// `spec/protocol.md §9.2` — health is `{"v":1,"id":"..."}` only; the manifest is the ID,
/// the name, the app index and the pair flag with no counts, timestamps or content; and a
/// caller that is not this machine gets a 403 from every route that says nothing at all
/// (`docs/plans/phase-1.md §2.2`).
#[tokio::test]
async fn test_spec_9_2_unauthenticated_leaks_nothing() {
    let root = tempfile::tempdir().unwrap();
    let handler = handler(&root);
    let id = handler.node().lock().unwrap().id().as_str().to_owned();

    let health = handler.handle(get("/api/v1/health")).await;
    assert_eq!(header(&health, &CONTENT_TYPE), "application/json");
    let health: serde_json::Value = serde_json::from_str(&body_of(health).await).unwrap();
    assert_eq!(health, serde_json::json!({ "v": 1, "id": id }));

    let manifest = handler.handle(get("/api/v1/manifest")).await;
    let manifest: serde_json::Value = serde_json::from_str(&body_of(manifest).await).unwrap();
    let keys: BTreeSet<&str> = manifest
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        ["v", "id", "name", "apps", "pair"].into_iter().collect()
    );
    assert_eq!(manifest["v"], 1);
    assert_eq!(manifest["id"], id);
    // No display name is set, so the Node ID stands in for it.
    assert_eq!(manifest["name"], id);
    assert_eq!(manifest["pair"], false);
    let apps = manifest["apps"].as_array().unwrap();
    let slugs: Vec<&str> = apps.iter().map(|a| a["slug"].as_str().unwrap()).collect();
    assert_eq!(slugs, ["animals", "hello", "sketch"]);
    for app in apps {
        let keys: BTreeSet<&str> = app
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert!(
            keys.is_subset(&["slug", "title", "icon"].into_iter().collect()),
            "{keys:?}"
        );
    }
    assert_eq!(apps[2]["title"], "Sketch");
    assert_eq!(apps[2]["icon"], "pencil-square");
    let text = manifest.to_string();
    for forbidden in ["count", "_at", "seq", "lam", "display_name", "profile"] {
        assert!(!text.contains(forbidden), "{forbidden} in {text}");
    }

    // Not this machine: 403 everywhere, and the body names the phase, not the node.
    for (path, _) in HOST_ROUTES {
        let response = handler
            .handle(with_peer(get(path), "192.168.1.5:40000"))
            .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{path}");
        let text = body_of(response).await;
        assert!(text.contains("loopback only"), "{path}: {text}");
        assert!(!text.contains(&id), "{path}: {text}");
        assert!(!text.contains("Sketch"), "{path}: {text}");
    }
    // Loopback with a Host that is not this machine — DNS rebinding — is refused too.
    let rebound = handler
        .handle(with_host(
            with_peer(get("/settings"), "127.0.0.1:40000"),
            "evil.example:8420",
        ))
        .await;
    assert_eq!(rebound.status(), StatusCode::FORBIDDEN);
    // Loopback with a loopback Host, or no peer at all (in-process), is this node.
    for request in [
        with_host(
            with_peer(get("/settings"), "127.0.0.1:40000"),
            "localhost:8420",
        ),
        with_peer(get("/settings"), "127.0.0.1:40000"),
        get("/settings"),
    ] {
        assert_eq!(handler.handle(request).await.status(), StatusCode::OK);
    }
}

// ---------------------------------------------------------------------------------------
// §9.3 — headers on every response
// ---------------------------------------------------------------------------------------

/// `spec/protocol.md §9.3` — the CSP, `nosniff` and `no-referrer` on every response, the
/// shell's being the default verbatim; `no-store` on everything but the embedded assets
/// and the skill documents; the shell never relaxes the default for itself.
#[tokio::test]
async fn test_spec_9_3_headers_present() {
    let root = tempfile::tempdir().unwrap();
    let handler = handler(&root);
    let mut requests: Vec<Request> = HOST_ROUTES.iter().map(|(path, _)| get(path)).collect();
    requests.push(request(Method::POST, "/"));
    requests.push(request(Method::GET, "/settings/apps/hello/seed"));
    requests.push(with_peer(get("/"), "10.0.0.1:1"));
    for request in requests {
        let path = request.uri().path().to_owned();
        let response = handler.handle(request).await;
        assert_eq!(
            header(&response, &X_CONTENT_TYPE_OPTIONS),
            "nosniff",
            "{path}"
        );
        assert_eq!(header(&response, &REFERRER_POLICY), "no-referrer", "{path}");
        let csp = header(&response, &CONTENT_SECURITY_POLICY).to_owned();
        assert!(!csp.is_empty(), "{path}: no CSP");
        let cacheable = path.starts_with("/static/") || path.starts_with("/skills/");
        let cache = header(&response, &CACHE_CONTROL);
        if cacheable && response.status() == StatusCode::OK {
            assert_eq!(cache, "no-cache", "{path}");
        } else {
            assert_eq!(cache, "no-store", "{path}");
        }
        if !path.starts_with("/a/") {
            assert_eq!(csp, DEFAULT_CSP, "{path}");
        }
    }

    // The shell's own pages carry no inline script or style, which is what lets §9.3
    // apply to them exactly as written.
    for path in [
        "/",
        "/settings",
        "/settings/apps",
        "/settings/data",
        "/settings/devices",
    ] {
        let html = body_of(handler.handle(get(path)).await).await;
        assert!(!html.contains("<script>"), "{path}: inline script");
        assert!(!html.contains(" style=\""), "{path}: inline style");
        assert!(!html.contains(" onclick="), "{path}: inline handler");
        assert!(html.contains("/static/htmx.min.js"), "{path}");
        assert!(html.contains("/static/shell.css"), "{path}");
        assert!(html.contains("<html lang=\"en\">"), "{path}");
    }
}

/// `spec/app-contract.md §5.4` — an app response carries exactly
/// `App::csp().header_for(origin)`, rendered against the request's `Host`, and never the
/// load-time `header()`.
#[tokio::test]
async fn test_app_response_carries_header_for_origin() {
    let root = tempfile::tempdir().unwrap();
    let handler = handler(&root);
    let (expected_host, expected_default, load_time) = {
        let node = handler.node().lock().unwrap();
        let sketch = node.app("sketch").unwrap();
        (
            sketch.csp().header_for("http://localhost:9999"),
            sketch.csp().header_for("http://127.0.0.1:8420"),
            sketch.csp().header().to_owned(),
        )
    };
    let with = handler
        .handle(with_host(get("/a/sketch/"), "localhost:9999"))
        .await;
    assert_eq!(header(&with, &CONTENT_SECURITY_POLICY), expected_host);
    assert!(
        expected_host
            .contains("script-src http://localhost:9999/a/sketch/ http://localhost:9999/static/")
    );
    assert_ne!(expected_host, load_time);

    // No Host, or one that could not go into a header: the node's own loopback origin.
    let without = handler.handle(get("/a/sketch/")).await;
    assert_eq!(header(&without, &CONTENT_SECURITY_POLICY), expected_default);
    let injected = handler
        .handle(with_host(get("/a/sketch/"), "x; script-src *"))
        .await;
    assert_eq!(
        header(&injected, &CONTENT_SECURITY_POLICY),
        expected_default
    );

    // Every response beneath the mount, including a 404 and a Tier 1 page, and no-store.
    for path in ["/a/sketch/style.css", "/a/sketch/missing.js", "/a/hello/"] {
        let response = handler.handle(get(path)).await;
        let csp = header(&response, &CONTENT_SECURITY_POLICY).to_owned();
        assert!(
            csp.contains("script-src http://127.0.0.1:8420/a/"),
            "{path}: {csp}"
        );
        assert!(
            csp.contains("http://127.0.0.1:8420/static/"),
            "{path}: {csp}"
        );
        assert_eq!(header(&response, &CACHE_CONTROL), "no-store", "{path}");
    }
}

// ---------------------------------------------------------------------------------------
// §5 — Tier 2 served as-is
// ---------------------------------------------------------------------------------------

/// `spec/app-contract.md §5` — `web/index.html` at the mount point, everything under
/// `web/` as-is with its content type, and nothing outside `web/` however the path is
/// spelled. The body of a large file arrives in frames, not as one buffer.
#[tokio::test]
async fn test_tier2_index_at_mount_and_nothing_outside_web() {
    let root = tempfile::tempdir().unwrap();
    let handler = handler(&root);
    let web = repo_apps_dir().join("sketch").join("web");

    let index = handler.handle(get("/a/sketch/")).await;
    assert_eq!(index.status(), StatusCode::OK);
    assert!(header(&index, &CONTENT_TYPE).starts_with("text/html"));
    assert_eq!(
        body_of(index).await,
        fs::read_to_string(web.join("index.html")).unwrap()
    );
    let css = handler.handle(get("/a/sketch/style.css")).await;
    assert!(
        header(&css, &CONTENT_TYPE).starts_with("text/css"),
        "{}",
        header(&css, &CONTENT_TYPE)
    );
    assert_eq!(
        body_of(css).await,
        fs::read_to_string(web.join("style.css")).unwrap()
    );
    let js = handler.handle(get("/a/sketch/app.js")).await;
    assert!(header(&js, &CONTENT_TYPE).contains("javascript"));

    for outside in [
        "/a/sketch/app.toml",
        "/a/sketch/../app.toml",
        "/a/sketch/%2e%2e/app.toml",
        "/a/sketch/..%2fapp.toml",
        "/a/sketch/README.md",
        "/a/sketch/../../Cargo.toml",
    ] {
        let response = handler.handle(get(outside)).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{outside}");
        let text = body_of(response).await;
        assert!(!text.contains("[app]"), "{outside} leaked the manifest");
        assert!(!text.contains("[workspace]"), "{outside}");
    }
}

/// ADR 0003 — `Response` bodies are streams. A 1 MiB file comes back as many frames of at
/// most 64 KiB, the first of them before the rest was read.
#[tokio::test]
async fn test_response_body_streams_without_buffering() {
    let root = tempfile::tempdir().unwrap();
    let apps = Node::open(root.path()).unwrap().paths().apps_dir();
    let big = vec![b'x'; 1024 * 1024];
    let dir = write_web_app(&apps, "big", &[]);
    fs::write(dir.join("web").join("big.bin"), &big).unwrap();
    let handler = handler(&root);

    let response = handler.handle(get("/a/big/big.bin")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body();
    let mut sizes = Vec::new();
    loop {
        let frame = std::future::poll_fn(|cx| Pin::new(&mut body).poll_frame(cx)).await;
        match frame {
            None => break,
            Some(Ok(frame)) => {
                if let Ok(data) = frame.into_data() {
                    sizes.push(data.len());
                }
            }
            Some(Err(error)) => panic!("{error}"),
        }
    }
    assert_eq!(sizes.iter().sum::<usize>(), big.len());
    assert!(sizes.len() >= 16, "{} frames", sizes.len());
    assert!(sizes.iter().all(|s| *s <= 64 * 1024), "{sizes:?}");
}

// ---------------------------------------------------------------------------------------
// §9.1 — solo mode
// ---------------------------------------------------------------------------------------

/// `spec/app-contract.md §2.2`, `spec/protocol.md §9.1` — the solo app owns `/`, with no
/// `/a/<slug>/` prefix, and in solo mode `header_for` is the default policy because the
/// app is the origin.
#[tokio::test]
async fn test_solo_mode_mounts_at_root() {
    let root = tempfile::tempdir().unwrap();
    let handler = solo(&root, "sketch");
    let web = repo_apps_dir().join("sketch").join("web");

    let index = handler.handle(with_host(get("/"), "127.0.0.1:8420")).await;
    assert_eq!(index.status(), StatusCode::OK);
    assert_eq!(
        body_of(index).await,
        fs::read_to_string(web.join("index.html")).unwrap()
    );
    let css = handler.handle(get("/style.css")).await;
    assert_eq!(css.status(), StatusCode::OK);
    assert_eq!(header(&css, &CONTENT_SECURITY_POLICY), DEFAULT_CSP);
    {
        let node = handler.node().lock().unwrap();
        let sketch = node.app("sketch").unwrap();
        assert_eq!(sketch.mount(), Some("/"));
        assert_eq!(
            sketch.csp().header_for("http://127.0.0.1:8420"),
            DEFAULT_CSP
        );
        assert!(node.app("hello").unwrap().mount().is_none());
    }
    // The manifest indexes the mounted app only.
    let manifest: serde_json::Value =
        serde_json::from_str(&body_of(handler.handle(get("/api/v1/manifest")).await).await)
            .unwrap();
    assert_eq!(manifest["apps"].as_array().unwrap().len(), 1);
    assert_eq!(manifest["apps"][0]["slug"], "sketch");
}

/// `spec/app-contract.md §2.2` — no launcher and no `/a/<slug>/` in solo mode; the shell's
/// own pages drop the launcher link.
#[tokio::test]
async fn test_launcher_absent_in_solo_mode() {
    let root = tempfile::tempdir().unwrap();
    let handler = solo(&root, "sketch");
    let index = body_of(handler.handle(get("/")).await).await;
    assert!(!index.contains("pv-launcher"), "{index}");
    assert!(
        !index.contains("pv-header"),
        "the shell leaked into the app: {index}"
    );
    // `/a/sketch/` is a path inside the solo app's `web/`, where nothing lives.
    let prefixed = handler.handle(get("/a/sketch/")).await;
    assert_eq!(prefixed.status(), StatusCode::NOT_FOUND);
    let settings = body_of(handler.handle(get("/settings")).await).await;
    assert!(!settings.contains("</svg> Apps</a>"), "{settings}");
    assert!(settings.contains("solo"), "{settings}");
}

/// `spec/protocol.md §9.1` — framework prefixes take precedence in solo mode, and each
/// shadowed route is a load-time warning naming the route and the prefix.
#[tokio::test]
async fn test_solo_mode_framework_prefix_wins() {
    let root = tempfile::tempdir().unwrap();
    let apps = Node::open(root.path()).unwrap().paths().apps_dir();
    write_web_app(
        &apps,
        "solo",
        &[
            ("web/settings/index.html", "<p>the app's settings</p>"),
            ("web/static/x.js", "alert(1)"),
            ("web/api/v1/health", "{\"fake\":true}"),
            ("web/play/index.html", "<p>play</p>"),
        ],
    );
    let handler = solo(&root, "solo");

    let settings = handler.handle(get("/settings")).await;
    assert_eq!(settings.status(), StatusCode::OK);
    let text = body_of(settings).await;
    assert!(text.contains("Settings"), "{text}");
    assert!(!text.contains("the app's settings"), "{text}");
    assert_eq!(
        handler.handle(get("/static/x.js")).await.status(),
        StatusCode::NOT_FOUND
    );
    let health = body_of(handler.handle(get("/api/v1/health")).await).await;
    let health: serde_json::Value = serde_json::from_str(&health).unwrap();
    assert_eq!(health["v"], 1, "{health}");
    assert!(health.get("fake").is_none(), "{health}");
    // The app's own routes are its.
    assert_eq!(handler.handle(get("/")).await.status(), StatusCode::OK);
    assert_eq!(handler.handle(get("/play/")).await.status(), StatusCode::OK);

    let shadowed: Vec<(String, &str)> = handler
        .report()
        .warnings
        .iter()
        .filter_map(|w| match w {
            Warning::RouteShadowed {
                slug,
                route,
                prefix,
            } if slug == "solo" => Some((route.clone(), *prefix)),
            _ => None,
        })
        .collect();
    assert_eq!(
        shadowed,
        [
            ("/api".to_owned(), "/api"),
            ("/settings".to_owned(), "/settings"),
            ("/static".to_owned(), "/static"),
        ]
    );
    let text = handler.report().warnings[0].to_string();
    assert!(text.contains("shadowed by the framework prefix"), "{text}");
    // The settings page surfaces them.
    let apps_page = body_of(handler.handle(get("/settings/apps")).await).await;
    assert!(
        apps_page.contains("shadowed by the framework prefix /settings"),
        "{apps_page}"
    );
}

// ---------------------------------------------------------------------------------------
// The read path — `echo >>` and the seed
// ---------------------------------------------------------------------------------------

/// `apps/hello/README.md` — a line appended by hand is visible on the next request, with
/// no restart: `refresh_app` runs per request (M5's stat, then a rebuild when stale).
#[tokio::test]
async fn test_hand_appended_line_visible_without_restart() {
    let root = tempfile::tempdir().unwrap();
    let handler = handler(&root);
    let before = body_of(handler.handle(get("/settings/apps")).await).await;
    assert!(before.contains("<code>profile</code>: 0 rows"), "{before}");

    let (path, dev) = {
        let node = handler.node().lock().unwrap();
        (
            node.paths().app_log(APP, node.id()),
            node.id().as_str().to_owned(),
        )
    };
    let ts = ts_offset_secs(-1);
    hand_append(
        &path,
        &event(
            1,
            1,
            &ts,
            &dev,
            "profile",
            "a",
            Some(r#"{"display_name":"Someone Else"}"#),
        ),
        "\n",
    );

    let after = body_of(handler.handle(get("/settings/apps")).await).await;
    assert!(after.contains("<code>profile</code>: 1 row "), "{after}");
    assert!(!after.contains("<code>profile</code>: 0 rows"), "{after}");
}

/// `spec/app-contract.md §9` — the seed offer is shown for a loaded app with an empty log,
/// and only a POST carrying `csrf()` loads it; a GET does nothing, a POST without the token
/// does nothing, and a second POST is refused because the log now holds events.
#[tokio::test]
async fn test_seed_offer_shown_and_only_a_post_loads_it() {
    let root = tempfile::tempdir().unwrap();
    let apps = Node::open(root.path()).unwrap().paths().apps_dir();
    write_app(
        &apps,
        "seeded",
        Some(&lua_manifest("seeded")),
        &[
            ("app.lua", ""),
            (
                "schema.sql",
                "CREATE TABLE profile (id VARCHAR PRIMARY KEY, display_name VARCHAR);",
            ),
            (
                "sample/seed.jsonl",
                "{\"op\":\"put\",\"tbl\":\"profile\",\"id\":\"a\",\"d\":{\"display_name\":\"Ada\"}}\n\
                 {\"op\":\"put\",\"tbl\":\"profile\",\"id\":\"b\",\"d\":{\"display_name\":\"Grace\"}}\n",
            ),
        ],
    );
    let handler = handler(&root);
    let action = "/settings/apps/seeded/seed";
    let log = {
        let node = handler.node().lock().unwrap();
        node.paths().app_log("seeded", node.id())
    };

    let page = body_of(handler.handle(get("/settings/apps")).await).await;
    assert!(page.contains(&format!("action=\"{action}\"")), "{page}");
    assert!(page.contains("name=\"_csrf\""), "{page}");
    assert!(page.contains("Load sample data"), "{page}");
    // Of the reference apps only animals ships a seed, so exactly two offers appear —
    // this app's and animals' — and nothing is offered for hello or sketch.
    assert_eq!(page.matches("Load sample data").count(), 2, "{page}");
    assert!(
        page.contains("action=\"/settings/apps/animals/seed\""),
        "{page}"
    );
    assert!(!page.contains("/settings/apps/hello/seed"), "{page}");
    assert!(!page.contains("/settings/apps/sketch/seed"), "{page}");

    // A GET is not the act.
    assert_eq!(
        handler.handle(get(action)).await.status(),
        StatusCode::METHOD_NOT_ALLOWED
    );
    assert_eq!(fs::read_to_string(&log).unwrap_or_default(), "");

    // A POST without the token is not the act either.
    let post = |body: String| {
        axum::http::Request::builder()
            .method(Method::POST)
            .uri(action)
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(body))
            .unwrap()
    };
    let forbidden = handler.handle(post("_csrf=nope".into())).await;
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);
    assert_eq!(fs::read_to_string(&log).unwrap_or_default(), "");
    let other = handler.csrf().token("/settings/apps/other/seed");
    let forbidden = handler.handle(post(format!("_csrf={other}"))).await;
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);

    // The act.
    let token = handler.csrf().token(action);
    let seeded = handler.handle(post(format!("_csrf={token}"))).await;
    assert_eq!(seeded.status(), StatusCode::SEE_OTHER);
    assert_eq!(header(&seeded, &LOCATION), "/settings/apps#app-seeded");
    let lines: Vec<serde_json::Value> = fs::read_to_string(&log)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["d"]["display_name"], "Ada");
    let page = body_of(handler.handle(get("/settings/apps")).await).await;
    assert!(page.contains("<code>profile</code>: 2 rows"), "{page}");
    // This app's offer is gone; animals', whose log is still empty, remains.
    assert!(!page.contains(&format!("action=\"{action}\"")), "{page}");
    assert_eq!(page.matches("Load sample data").count(), 1, "{page}");

    // Never over existing events.
    let refused = handler.handle(post(format!("_csrf={token}"))).await;
    assert_eq!(refused.status(), StatusCode::CONFLICT);
    let text = body_of(refused).await;
    assert!(text.contains("already holds 2 event"), "{text}");
    assert_eq!(fs::read_to_string(&log).unwrap().lines().count(), 2);
}

// ---------------------------------------------------------------------------------------
// The shell
// ---------------------------------------------------------------------------------------

/// `spec/data-dictionary.md §3.4` — an app whose folder is gone stays in the launcher as
/// unavailable, with the reason, rather than pretending it is gone.
#[tokio::test]
async fn test_launcher_shows_a_missing_folder_as_unavailable() {
    let root = tempfile::tempdir().unwrap();
    let apps = Node::open(root.path()).unwrap().paths().apps_dir();
    let dir = write_web_app(&apps, "gone", &[]);
    {
        let (node, _) = open(&root);
        assert!(node.app("gone").is_some());
    }
    fs::remove_dir_all(&dir).unwrap();
    let handler = handler(&root);
    assert!(handler.report().missing.contains(&"gone".to_owned()));

    let launcher = body_of(handler.handle(get("/")).await).await;
    assert!(launcher.contains("pv-launcher"), "{launcher}");
    assert!(launcher.contains("href=\"/a/sketch/\""), "{launcher}");
    assert!(
        launcher.contains("gone — unavailable: folder missing"),
        "{launcher}"
    );
    assert!(!launcher.contains("href=\"/a/gone/\""), "{launcher}");
    assert_eq!(
        handler.handle(get("/a/gone/")).await.status(),
        StatusCode::NOT_FOUND
    );
    // Icons are inlined SVG with the attributes docs/icons.md requires, never a font.
    assert!(launcher.contains("<svg class=\"pv-icon\""), "{launcher}");
    assert!(launcher.contains("focusable=\"false\""));
    assert!(
        !launcher.contains("bi-"),
        "a vendored class leaked: {launcher}"
    );
}

/// The four settings pages render what `docs/plans/phase-1.md` M6 lists: identity, the
/// installed apps with their warnings and errors, the data directory with backup
/// instructions, and this node's own device row with the Phase 2 note.
#[tokio::test]
async fn test_settings_pages_render_the_node() {
    let root = tempfile::tempdir().unwrap();
    let apps = Node::open(root.path()).unwrap().paths().apps_dir();
    // A permission widening and an icon the set lacks, both surfaced on the apps page.
    write_app(
        &apps,
        "wide",
        Some(&format!(
            "{}icon = \"no-such-icon\"\n[permissions]\nsql = true\n",
            lua_manifest("wide")
        )),
        &[("app.lua", "")],
    );
    // A broken folder, refused loudly.
    write_app(&apps, "broken", Some("not toml at all ["), &[]);
    let handler = handler(&root);
    let (id, data_dir) = {
        let node = handler.node().lock().unwrap();
        (
            node.id().as_str().to_owned(),
            node.paths().data_dir().display().to_string(),
        )
    };

    let node_page = body_of(handler.handle(get("/settings")).await).await;
    assert!(node_page.contains(&id), "{node_page}");
    assert!(node_page.contains("pv/1"), "{node_page}");
    assert!(node_page.contains("host —"), "{node_page}");
    assert!(node_page.contains("No alerts"), "{node_page}");

    let apps_page = body_of(handler.handle(get("/settings/apps")).await).await;
    for expected in [
        "id=\"app-hello\"",
        "id=\"app-sketch\"",
        "href=\"/a/sketch/\"",
        "ad-hoc read-only SQL",
        "not in the vendored Bootstrap Icons set",
        "Not loaded at startup",
        "<code>broken</code>",
        "tier 3 — full replay",
        "no schema.sql — the event log is the store",
    ] {
        assert!(apps_page.contains(expected), "{expected}\n{apps_page}");
    }

    let data_page = body_of(handler.handle(get("/settings/data")).await).await;
    assert!(
        data_page.contains(&privatium_core::icons::escape(&data_dir)),
        "{data_page}"
    );
    assert!(
        data_page.contains("Copy the <code>data</code> folder"),
        "{data_page}"
    );

    let devices = body_of(handler.handle(get("/settings/devices")).await).await;
    assert!(devices.contains(&id), "{devices}");
    assert!(devices.contains("this node"), "{devices}");
    assert!(devices.contains("Pairing arrives in Phase 2"), "{devices}");
}
