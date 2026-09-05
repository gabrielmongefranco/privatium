// Project:  Privatium™  |  File: crates/privatium-core/tests/data.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-03  |  Modified: 2026-09-05
// Summary:  The data API against spec/data-api.md and docs/plans/phase-1.md M9, every test
//           through core::handle with no listener: the client's four fields and nothing
//           stamped (§2, PV304), batches all or nothing with the offending index, the
//           limits, ad-hoc SQL behind its permission with bound parameters only, `$name`
//           views, the NDJSON of raw lines, the row endpoint, SSE with no gap across a
//           reconnect and frames that arrive while the stream is open, resync on a rebuilt
//           cache, pv.on('append') for an API append, a tombstoned id refused, DECIMAL as a
//           string end to end, `sys.v_*` readable, and sketch with no schema.sql at all.

// AGENTS.md, Style: unwrap() is permitted in tests, and a test that hides a failure
// behind `?` is worse than one that panics with a line number.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::fs;
use std::path::PathBuf;
use std::pin::Pin;
use std::time::Duration;

use axum::body::{Body, HttpBody as _, to_bytes};
use axum::http::header::{ALLOW, CONTENT_LENGTH, CONTENT_TYPE, RETRY_AFTER};
use axum::http::{Method, StatusCode};
use common::{
    event, hand_append, log_lines, lua_manifest, repo_apps_dir, ts_offset_secs, write_app,
    write_web_app,
};
use privatium_core::{AppRoot, Handler, LoadReport, Node, Request, Response};
use serde_json::{Value, json};

/// A schema exercising every typing rule of `spec/data-dictionary.md §2.1`, a `NOT NULL`,
/// a `CHECK`, a plain view, a computed view and a `$name` view.
const FILL_DDL: &str = "CREATE TABLE fill (
    id           VARCHAR PRIMARY KEY,
    drug         VARCHAR NOT NULL,
    copay_amount DECIMAL(18,2) CHECK (copay_amount >= 0),
    count        BIGINT,
    ok           BOOLEAN,
    tags         VARCHAR[],
    filled_on    DATE
);
CREATE VIEW v_fill AS SELECT id, drug, copay_amount, count, ok, tags, filled_on FROM fill;
CREATE VIEW v_stats AS SELECT count(*) AS n, decimal_sum(copay_amount) AS total FROM fill;
CREATE VIEW v_since AS SELECT id, drug FROM fill WHERE filled_on >= $since ORDER BY id;";

/// A Tier 2 manifest with the SQL permission.
fn sql_manifest(slug: &str) -> String {
    format!(
        "[app]\nslug = \"{slug}\"\ntitle = \"{slug}\"\nversion = \"1.0.0\"\napi = 1\ntier = \"web\"\n\n[permissions]\nsql = true\n"
    )
}

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

/// The `meds` app: Tier 2, `FILL_DDL`, SQL permitted.
fn with_meds(root: &tempfile::TempDir) -> Handler {
    let apps = privatium_core::Paths::rooted(root.path()).apps_dir();
    write_app(
        &apps,
        "meds",
        Some(&sql_manifest("meds")),
        &[
            ("web/index.html", "<!doctype html><title>meds</title>"),
            ("schema.sql", FILL_DDL),
        ],
    );
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

fn post_json(path: &str, value: &Value) -> Request {
    axum::http::Request::builder()
        .method(Method::POST)
        .uri(path)
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(value.to_string()))
        .unwrap()
}

fn post_raw(path: &str, content_type: Option<&str>, body: &str) -> Request {
    let mut builder = axum::http::Request::builder()
        .method(Method::POST)
        .uri(path);
    if let Some(content_type) = content_type {
        builder = builder.header(CONTENT_TYPE, content_type);
    }
    builder.body(Body::from(body.to_owned())).unwrap()
}

fn with_header(mut request: Request, name: &'static str, value: &str) -> Request {
    request.headers_mut().insert(name, value.parse().unwrap());
    request
}

async fn body_of(response: Response) -> String {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Status and the body as JSON — or as a string when it is not JSON.
async fn json_of(response: Response) -> (StatusCode, Value) {
    let status = response.status();
    let text = body_of(response).await;
    let value = serde_json::from_str(&text).unwrap_or(Value::String(text));
    (status, value)
}

fn header<'a>(response: &'a Response, name: &axum::http::HeaderName) -> &'a str {
    response
        .headers()
        .get(name)
        .map(|v| v.to_str().unwrap())
        .unwrap_or_default()
}

/// The next SSE frame of a stream body, within `secs`, as `(event, data)`.
async fn next_frame(body: &mut Body, secs: u64) -> Option<(String, String)> {
    let frame = tokio::time::timeout(
        Duration::from_secs(secs),
        std::future::poll_fn(|cx| Pin::new(&mut *body).poll_frame(cx)),
    )
    .await
    .ok()??
    .ok()?;
    let text = String::from_utf8_lossy(&frame.into_data().ok()?).into_owned();
    let mut event = String::new();
    let mut data = String::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("event: ") {
            event = rest.to_owned();
        } else if let Some(rest) = line.strip_prefix("data: ") {
            data = rest.to_owned();
        }
    }
    Some((event, data))
}

fn log_path(handler: &Handler, slug: &str) -> PathBuf {
    let node = handler.node().lock().unwrap();
    node.paths().app_log(slug, node.id())
}

fn node_id(handler: &Handler) -> String {
    handler.node().lock().unwrap().id().as_str().to_owned()
}

/// Every line of an app's log, verbatim.
fn raw_lines(handler: &Handler, slug: &str) -> Vec<String> {
    fs::read_to_string(log_path(handler, slug))
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// Set a `sys_setting` (`spec/data-dictionary.md §3.6`) and refresh `_sys`.
fn set_setting(handler: &Handler, key: &str, value: &str) {
    let mut node = handler.node().lock().unwrap();
    let at = privatium_core::log::now();
    node.sys_log_mut()
        .put(
            privatium_core::sys::SETTING,
            key,
            &json!({ "value": value, "updated_at": at }),
        )
        .unwrap();
    node.refresh().unwrap();
}

fn stroke(id: Option<&str>) -> Value {
    let mut ev = json!({ "op": "put", "tbl": "stroke", "d": { "points": [[0, 0], [4, 9]], "color": "#00274C", "width": 3 } });
    if let Some(id) = id {
        ev["id"] = Value::String(id.to_owned());
    }
    ev
}

fn ulid() -> String {
    ulid::Ulid::generate().to_string()
}

// ---------------------------------------------------------------------------------------
// §2 — write
// ---------------------------------------------------------------------------------------

/// `spec/data-api.md §2`, `PV304` — a client supplies `op`, `tbl`, `id` and `d` only. A
/// request that sets `seq`, `lam`, `ts`, `dev` or `app` is refused naming the field and
/// the rule; so is a field that is not one of the four; nothing reaches the log.
#[tokio::test]
async fn test_spec_data_2_client_cannot_set_seq() {
    let root = tempfile::tempdir().unwrap();
    let handler = handler(&root);
    for stamped in ["seq", "lam", "ts", "dev", "app"] {
        let mut ev = stroke(None);
        ev[stamped] = json!("x");
        let (status, body) = json_of(
            handler
                .handle(post_json(
                    "/a/sketch/api/events",
                    &json!({ "events": [ev] }),
                ))
                .await,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{stamped}: {body}");
        let error = body["error"].as_str().unwrap();
        assert!(error.contains("PV304"), "{stamped}: {error}");
        assert!(error.contains(&format!("`{stamped}`")), "{error}");
        assert_eq!(body["index"], 0);
    }
    let mut ev = stroke(None);
    ev["extra"] = json!(1);
    let (status, body) = json_of(
        handler
            .handle(post_json(
                "/a/sketch/api/events",
                &json!({ "events": [stroke(None), ev] }),
            ))
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["index"], 1);
    assert!(
        body["error"].as_str().unwrap().contains("`extra`"),
        "{body}"
    );
    assert!(
        raw_lines(&handler, "sketch").is_empty(),
        "nothing was written"
    );
}

/// `spec/data-api.md §2` — a batch is all or nothing: a `NOT NULL` or `CHECK` violation,
/// a value that is not its type, or an id that is not a ULID anywhere in the batch refuses
/// the whole batch with the offending index, and the log holds none of it; a clean batch
/// lands with contiguous events, minted ids and the high-water mark.
#[tokio::test]
async fn test_spec_data_2_batch_atomic() {
    let root = tempfile::tempdir().unwrap();
    let handler = with_meds(&root);
    let ok =
        json!({ "op": "put", "tbl": "fill", "d": { "drug": "Example", "copay_amount": "12.50" } });
    let cases: Vec<(Value, &str)> = vec![
        (
            json!({ "op": "put", "tbl": "fill", "d": { "copay_amount": "1.00" } }),
            "NOT NULL",
        ),
        (
            json!({ "op": "put", "tbl": "fill", "d": { "drug": "X", "copay_amount": "1.234" } }),
            "copay_amount",
        ),
        (
            json!({ "op": "put", "tbl": "fill", "d": { "drug": "X", "copay_amount": "-1.00" } }),
            "CHECK",
        ),
        (
            json!({ "op": "put", "tbl": "fill", "id": "not-a-ulid", "d": { "drug": "X" } }),
            "ULID",
        ),
        (json!({ "op": "del", "tbl": "fill" }), "`id`"),
    ];
    for (bad, needle) in cases {
        let (status, body) = json_of(
            handler
                .handle(post_json(
                    "/a/meds/api/events",
                    &json!({ "events": [ok.clone(), bad.clone()] }),
                ))
                .await,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}: {body}");
        assert_eq!(body["index"], 1, "{bad}: {body}");
        assert!(
            body["error"].as_str().unwrap().contains(needle),
            "{bad}: {body}"
        );
    }
    assert!(
        raw_lines(&handler, "meds").is_empty(),
        "nothing was written"
    );

    let (status, body) = json_of(
        handler
            .handle(post_json(
                "/a/meds/api/events",
                &json!({ "events": [ok.clone(), ok.clone()] }),
            ))
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["appended"], 2);
    assert_eq!(body["lam"], 2);
    let ids = body["ids"].as_array().unwrap();
    assert_eq!(ids.len(), 2);
    for id in ids {
        assert!(
            ulid::Ulid::from_string(id.as_str().unwrap()).is_ok(),
            "{id}"
        );
    }
    let lines = log_lines(&log_path(&handler, "meds"));
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["seq"], 1);
    assert_eq!(lines[1]["seq"], 2);
    assert_eq!(lines[0]["ts"], lines[1]["ts"], "one batch, one ts");
    assert_eq!(lines[0]["id"], ids[0]);
    let (status, body) = json_of(handler.handle(get("/a/meds/api/q/v_fill")).await).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"].as_array().unwrap().len(), 2);
}

/// `spec/data-api.md §2`, `§7` — at most 1000 events and 4 MB per request by default, and
/// `api.max_batch` in `sys_setting` moves the first.
#[tokio::test]
async fn test_spec_data_2_batch_limits() {
    let root = tempfile::tempdir().unwrap();
    let handler = handler(&root);
    let many: Vec<Value> = (0..1001).map(|_| stroke(None)).collect();
    let (status, body) = json_of(
        handler
            .handle(post_json(
                "/a/sketch/api/events",
                &json!({ "events": many }),
            ))
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("api.max_batch"),
        "{body}"
    );

    // A declared length past the limit is refused before a byte is read; a body that
    // grows past it while being read is refused where it does.
    let declared = with_header(
        post_json("/a/sketch/api/events", &json!({ "events": [] })),
        "content-length",
        "5000000",
    );
    let response = handler.handle(declared).await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let big = json!({ "events": [], "pad": "x".repeat(4 * 1024 * 1024 + 1) });
    let response = handler
        .handle(post_json("/a/sketch/api/events", &big))
        .await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(header(&response, &CONTENT_TYPE), "application/json");

    set_setting(&handler, "api.max_batch", "5");
    let six: Vec<Value> = (0..6).map(|_| stroke(None)).collect();
    let (status, _) = json_of(
        handler
            .handle(post_json("/a/sketch/api/events", &json!({ "events": six })))
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let five: Vec<Value> = (0..5).map(|_| stroke(None)).collect();
    let (status, body) = json_of(
        handler
            .handle(post_json(
                "/a/sketch/api/events",
                &json!({ "events": five }),
            ))
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["appended"], 5);
    assert_eq!(raw_lines(&handler, "sketch").len(), 5);
    assert_eq!(CONTENT_LENGTH.as_str(), "content-length");
}

/// `spec/protocol.md §4.6` — a minted id that keyed a deleted row is never the key of
/// another: a put under it is 409 with the index, whether the tombstone is in the cache
/// or earlier in the same batch. A repeated del is a replay and lands; a fresh ULID does.
#[tokio::test]
async fn test_spec_4_6_tombstoned_minted_id_refused() {
    let root = tempfile::tempdir().unwrap();
    let handler = handler(&root);
    let post = |events: Vec<Value>| post_json("/a/sketch/api/events", &json!({ "events": events }));
    let x = ulid();
    let (status, _) = json_of(handler.handle(post(vec![stroke(Some(&x))])).await).await;
    assert_eq!(status, StatusCode::OK);
    let del = json!({ "op": "del", "tbl": "stroke", "id": x });
    let (status, _) = json_of(handler.handle(post(vec![del.clone()])).await).await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = json_of(handler.handle(post(vec![stroke(Some(&x))])).await).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["index"], 0);
    assert!(body["error"].as_str().unwrap().contains("§4.6"), "{body}");

    // The outbox may resend a del that already landed: idempotent, not a conflict.
    let (status, body) = json_of(handler.handle(post(vec![del.clone()])).await).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Deleted earlier in the same batch.
    let y = ulid();
    let (status, _) = json_of(handler.handle(post(vec![stroke(Some(&y))])).await).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = json_of(
        handler
            .handle(post(vec![
                json!({ "op": "del", "tbl": "stroke", "id": y }),
                stroke(Some(&y)),
            ]))
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["index"], 1);

    let (status, _) = json_of(handler.handle(post(vec![stroke(Some(&ulid()))])).await).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(raw_lines(&handler, "sketch").len(), 5);
}

/// `spec/data-api.md §2` — a POST may name the node and the app it is for, and one that
/// names another node or another app is 409 before anything is appended. The right
/// names land, and so does a body that names neither.
#[tokio::test]
async fn test_spec_data_2_post_naming_another_node_or_app_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let handler = handler(&root);
    let id = handler.node().lock().unwrap().id().as_str().to_owned();
    let post = |extra: Value| {
        let mut body = json!({ "events": [stroke(None)] });
        for (key, value) in extra.as_object().unwrap() {
            body[key] = value.clone();
        }
        post_json("/a/sketch/api/events", &body)
    };

    let (status, body) = json_of(
        handler
            .handle(post(json!({ "node": "someone", "app": "sketch" })))
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("someone"),
        "{body}"
    );
    let (status, body) = json_of(
        handler
            .handle(post(json!({ "node": id, "app": "other" })))
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body["error"].as_str().unwrap().contains("other"), "{body}");
    assert_eq!(
        raw_lines(&handler, "sketch").len(),
        0,
        "nothing was appended"
    );

    let (status, body) = json_of(
        handler
            .handle(post(json!({ "node": id, "app": "sketch" })))
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = json_of(handler.handle(post(json!({}))).await).await;
    assert_eq!(status, StatusCode::OK, "the names are optional: {body}");
    assert_eq!(raw_lines(&handler, "sketch").len(), 2);
}

/// `spec/lua-api.md §3.4` — an API append is this node's own, so a Tier 1 app's
/// `pv.on('append')` fires for it, after the response is decided.
#[tokio::test]
async fn test_api_append_fires_pv_on_append() {
    let root = tempfile::tempdir().unwrap();
    let apps = privatium_core::Paths::rooted(root.path()).apps_dir();
    fs::write(
        root.path().join("config.toml"),
        "[lua]\npool_size = 1\nmax_instructions = 5000000\nmax_memory_mb = 16\nmax_seconds = 20\n",
    )
    .unwrap();
    write_app(
        &apps,
        "react",
        Some(&lua_manifest("react")),
        &[
            (
                "app.lua",
                "local pv = require 'privatium'\n\
                 pv.on('append', function(ev)\n\
                   if ev.tbl == 'thing' then pv.append('echo', ev.id, { seq = tostring(ev.seq), name = ev.d.name }) end\n\
                 end)\n\
                 pv.get('/', function() return pv.text('ok') end)\n",
            ),
            (
                "schema.sql",
                "CREATE TABLE thing (id VARCHAR PRIMARY KEY, name VARCHAR);\n\
                 CREATE TABLE echo (id VARCHAR PRIMARY KEY, seq BIGINT, name VARCHAR);",
            ),
        ],
    );
    let handler = handler(&root);
    let id = ulid();
    let (status, body) = json_of(
        handler
            .handle(post_json(
                "/a/react/api/events",
                &json!({ "events": [{ "op": "put", "tbl": "thing", "id": id, "d": { "name": "Ada" } }] }),
            ))
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let lines = log_lines(&log_path(&handler, "react"));
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert_eq!(lines[1]["tbl"], "echo");
    assert_eq!(lines[1]["id"], id);
    assert_eq!(lines[1]["d"]["seq"], "1");
    assert_eq!(lines[1]["d"]["name"], "Ada");
    // The reaction is a row like any other, readable through the API.
    let (status, body) = json_of(
        handler
            .handle(get(&format!("/a/react/api/row/echo/{id}")))
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["d"]["seq"], "1");
}

// ---------------------------------------------------------------------------------------
// §1 — read
// ---------------------------------------------------------------------------------------

/// `spec/data-api.md §1` — `/api/sql` needs `permissions.sql = true`; with it, a single
/// `SELECT` or `WITH … SELECT` on the sandboxed connection; anything else is refused;
/// a POST that is not JSON is refused before it is read.
#[tokio::test]
async fn test_spec_data_1_sql_requires_permission() {
    let root = tempfile::tempdir().unwrap();
    let handler = with_meds(&root);
    let query = |slug: &str, sql: &str| {
        post_json(
            &format!("/a/{slug}/api/sql"),
            &json!({ "sql": sql, "params": [] }),
        )
    };
    let (status, body) = json_of(handler.handle(query("sketch", "SELECT 1")).await).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("permissions.sql"),
        "{body}"
    );

    let (status, body) = json_of(handler.handle(query("meds", "SELECT 1 AS one")).await).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"], json!([{ "one": 1 }]));
    assert_eq!(body["columns"], json!([{ "name": "one", "type": null }]));
    assert_eq!(body["lam"], 0);
    let (status, body) = json_of(
        handler
            .handle(query("meds", "WITH x AS (SELECT 2 AS a) SELECT a FROM x"))
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"], json!([{ "a": 2 }]));

    for refused in [
        "DELETE FROM fill",
        "INSERT INTO fill (id) VALUES ('x')",
        "PRAGMA journal_mode",
        "VALUES (1)",
        "EXPLAIN SELECT 1",
        "SELECT 1; SELECT 2",
        "SELECT load_extension('x')",
        "SELECT * FROM nope",
        "DETACH sys",
    ] {
        let (status, body) = json_of(handler.handle(query("meds", refused)).await).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}: {body}");
    }
    let response = handler
        .handle(post_raw(
            "/a/meds/api/sql",
            Some("text/plain"),
            "{\"sql\":\"SELECT 1\"}",
        ))
        .await;
    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let response = handler
        .handle(post_raw("/a/meds/api/sql", None, "{\"sql\":\"SELECT 1\"}"))
        .await;
    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let response = handler.handle(get("/a/meds/api/sql")).await;
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(header(&response, &ALLOW), "POST");
    let (status, body) = json_of(
        handler
            .handle(post_raw(
                "/a/meds/api/sql",
                Some("application/json"),
                "not json",
            ))
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(raw_lines(&handler, "meds").len(), 0);
}

/// `spec/data-api.md §1` — a `?` count that does not match `params` is refused, never
/// substituted; a `$name` view binds from the query string, an unknown key is refused
/// naming the view's placeholders, and paging is clamped and checked.
#[tokio::test]
async fn test_spec_data_1_param_count_mismatch_rejected() {
    let root = tempfile::tempdir().unwrap();
    let handler = with_meds(&root);
    let sql = |params: Value| {
        post_json(
            "/a/meds/api/sql",
            &json!({ "sql": "SELECT ? AS a, ? AS b", "params": params }),
        )
    };
    for short in [json!(["1"]), json!([]), json!(["1", "2", "3"])] {
        let (status, body) = json_of(handler.handle(sql(short.clone())).await).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{short}: {body}");
        assert!(
            body["error"].as_str().unwrap().contains("placeholder"),
            "{body}"
        );
    }
    let (status, body) = json_of(handler.handle(sql(json!(["1", 2]))).await).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"], json!([{ "a": "1", "b": 2 }]));
    let (status, body) = json_of(handler.handle(sql(json!([{}, 1]))).await).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("params[0]"),
        "{body}"
    );

    // `$since` on a view.
    let events = json!({ "events": [
        { "op": "put", "tbl": "fill", "d": { "drug": "Old", "filled_on": "2025-01-01" } },
        { "op": "put", "tbl": "fill", "d": { "drug": "New", "filled_on": "2026-06-01" } },
    ] });
    let (status, _) = json_of(
        handler
            .handle(post_json("/a/meds/api/events", &events))
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = json_of(
        handler
            .handle(get("/a/meds/api/q/v_since?since=2026-01-01"))
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["view"], "v_since");
    assert_eq!(body["rows"].as_array().unwrap().len(), 1);
    assert_eq!(body["rows"][0]["drug"], "New");
    assert_eq!(body["lam"], 2);
    let (status, body) = json_of(handler.handle(get("/a/meds/api/q/v_since")).await).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"], json!([]), "an unbound placeholder is NULL");
    let (status, body) = json_of(
        handler
            .handle(get("/a/meds/api/q/v_since?until=2026-01-01"))
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["error"].as_str().unwrap().contains("since"), "{body}");
    let (status, _) = json_of(handler.handle(get("/a/meds/api/q/v_nope")).await).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = json_of(handler.handle(get("/a/meds/api/q/v_fill?limit=abc")).await).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, body) = json_of(
        handler
            .handle(get("/a/meds/api/q/v_fill?limit=20000&offset=1"))
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"].as_array().unwrap().len(), 1, "offset 1 of 2");
    let (status, body) = json_of(handler.handle(get("/a/meds/api/q/v_fill?limit=0")).await).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"], json!([]));
}

/// `spec/data-api.md §1`, `§5`, `spec/data-dictionary.md §2.1` — a `DECIMAL` is a JSON
/// string end to end: a number sent by a client lands as digits at the declared scale, a
/// `BIGINT` as digits, a `BOOLEAN` as a boolean, a `JSON` column as its value; through a
/// view and an alias the declaration still types the column; a computed `count(*)` is a
/// number and `decimal_sum()` a string; and `pv.js` converts nothing.
#[tokio::test]
async fn test_spec_data_5_decimal_stays_string() {
    let root = tempfile::tempdir().unwrap();
    let handler = with_meds(&root);
    let (status, body) = json_of(
        handler
            .handle(post_json(
                "/a/meds/api/events",
                &json!({ "events": [{ "op": "put", "tbl": "fill", "d": {
                    "drug": "Example", "copay_amount": 12.5, "count": 3, "ok": "yes",
                    "tags": ["a", "b"], "filled_on": "3/9/2026", "note": "kept as is"
                } }] }),
            ))
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let line = &raw_lines(&handler, "meds")[0];
    assert!(line.contains("\"copay_amount\":\"12.50\""), "{line}");
    assert!(line.contains("\"count\":\"3\""), "{line}");
    assert!(line.contains("\"ok\":true"), "{line}");
    assert!(line.contains("\"filled_on\":\"2026-03-09\""), "{line}");
    assert!(line.contains("\"note\":\"kept as is\""), "{line}");

    let (status, body) = json_of(handler.handle(get("/a/meds/api/q/v_fill")).await).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let row = &body["rows"][0];
    assert_eq!(row["copay_amount"], Value::String("12.50".into()));
    assert_eq!(row["count"], Value::String("3".into()));
    assert_eq!(row["ok"], Value::Bool(true));
    assert_eq!(row["tags"], json!(["a", "b"]));
    assert_eq!(row["filled_on"], "2026-03-09");
    let columns = body["columns"].as_array().unwrap();
    assert!(
        columns.contains(&json!({ "name": "copay_amount", "type": "DECIMAL(18,2)" })),
        "{columns:?}"
    );
    assert!(columns.contains(&json!({ "name": "count", "type": "BIGINT" })));

    let (status, body) = json_of(handler.handle(get("/a/meds/api/q/v_stats")).await).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"][0]["n"], json!(1));
    assert_eq!(body["rows"][0]["total"], Value::String("12.50".into()));
    let (status, body) = json_of(
        handler
            .handle(post_json(
                "/a/meds/api/sql",
                &json!({ "sql": "SELECT count AS c, copay_amount * 2 AS twice FROM fill" }),
            ))
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"][0]["c"], Value::String("3".into()));
    assert_eq!(
        body["rows"][0]["twice"],
        json!(25.0),
        "a computed float is a float — PV302 territory"
    );

    let pv = body_of(handler.handle(get("/static/pv.js")).await).await;
    for conversion in ["Number(", "parseFloat(", "parseInt("] {
        assert!(!pv.contains(conversion), "pv.js converts with {conversion}");
    }
}

/// `spec/data-api.md §1` — `/api/events` is the log's own lines, byte for byte, in
/// `(lam, ts, dev)` order, filtered by `tbl`, `id` and `after` and paged; `/api/row` is
/// the row's winning line, 404 for an absent or tombstoned id.
#[tokio::test]
async fn test_api_events_ndjson_byte_identical_and_row() {
    let root = tempfile::tempdir().unwrap();
    let handler = handler(&root);
    let ids: Vec<String> = (0..3).map(|_| ulid()).collect();
    for id in &ids {
        let (status, _) = json_of(
            handler
                .handle(post_json(
                    "/a/sketch/api/events",
                    &json!({ "events": [stroke(Some(id))] }),
                ))
                .await,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }
    let (status, _) = json_of(
        handler
            .handle(post_json(
                "/a/sketch/api/events",
                &json!({ "events": [{ "op": "del", "tbl": "stroke", "id": ids[1] }] }),
            ))
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let disk = fs::read_to_string(log_path(&handler, "sketch")).unwrap();
    let lines: Vec<&str> = disk.lines().collect();
    assert_eq!(lines.len(), 4);

    let response = handler.handle(get("/a/sketch/api/events")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(header(&response, &CONTENT_TYPE), "application/x-ndjson");
    assert_eq!(body_of(response).await, disk, "byte-identical, same order");
    let body = body_of(
        handler
            .handle(get("/a/sketch/api/events?tbl=stroke&after=2"))
            .await,
    )
    .await;
    assert_eq!(body, format!("{}\n{}\n", lines[2], lines[3]));
    let body = body_of(
        handler
            .handle(get(&format!("/a/sketch/api/events?id={}", ids[1])))
            .await,
    )
    .await;
    assert_eq!(body, format!("{}\n{}\n", lines[1], lines[3]));
    let body = body_of(
        handler
            .handle(get("/a/sketch/api/events?limit=1&offset=1"))
            .await,
    )
    .await;
    assert_eq!(body, format!("{}\n", lines[1]));
    let body = body_of(handler.handle(get("/a/sketch/api/events?tbl=other")).await).await;
    assert_eq!(body, "");
    let response = handler.handle(get("/a/sketch/api/events?after=x")).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let response = handler
        .handle(get(&format!("/a/sketch/api/row/stroke/{}", ids[0])))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(header(&response, &CONTENT_TYPE), "application/json");
    assert_eq!(body_of(response).await, lines[0]);
    for absent in [
        format!("/a/sketch/api/row/stroke/{}", ids[1]),
        "/a/sketch/api/row/stroke/nope".to_owned(),
        "/a/sketch/api/row/other/x".to_owned(),
    ] {
        let response = handler.handle(get(&absent)).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{absent}");
    }
    let response = handler.handle(get("/a/sketch/api/row/bad-name/x")).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

/// `spec/protocol.md §4.1`, `spec/data-api.md §1`, `§2` — a batch the API appended is
/// one batch of the log, its first line carrying the count; and a batch that reached the
/// disk short — the node crashed between the write and the disk, then restarted — is
/// served by nothing: not `/api/events`, not `/api/row`, not the tables, though its lines
/// are still in the file.
#[tokio::test]
async fn test_spec_4_1_short_batch_is_served_by_nothing() {
    let root = tempfile::tempdir().unwrap();
    let ids: Vec<String> = (0..3).map(|_| ulid()).collect();
    let path = {
        let before = handler(&root);
        let events: Vec<Value> = ids.iter().map(|id| stroke(Some(id))).collect();
        let (status, _) = json_of(
            before
                .handle(post_json(
                    "/a/sketch/api/events",
                    &json!({ "events": events }),
                ))
                .await,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = json_of(
            before
                .handle(post_json(
                    "/a/sketch/api/events",
                    &json!({ "events": [stroke(Some(&ulid()))] }),
                ))
                .await,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        log_path(&before, "sketch")
    };
    let disk = fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = disk.lines().collect();
    assert_eq!(lines.len(), 4);
    assert!(lines[0].contains("\"batch\":3"), "{}", lines[0]);
    assert!(!lines[1].contains("\"batch\""), "{}", lines[1]);
    assert!(!lines[3].contains("\"batch\""), "{}", lines[3]);
    // What `/api/events` serves is the file, batch and all, while the batch is whole.
    let whole = handler(&root);
    assert_eq!(
        body_of(whole.handle(get("/a/sketch/api/events")).await).await,
        disk
    );
    drop(whole);

    // The crash: the third line of the batch never reached the disk.
    fs::write(&path, format!("{}\n{}\n", lines[0], lines[1])).unwrap();
    let after = handler(&root);
    let body = body_of(after.handle(get("/a/sketch/api/events")).await).await;
    assert_eq!(body, "", "a short batch is not served");
    let response = after
        .handle(get(&format!("/a/sketch/api/row/stroke/{}", ids[0])))
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    // The lines stay, and the next append lands past them.
    assert_eq!(fs::read_to_string(&path).unwrap().lines().count(), 2);
    let (status, out) = json_of(
        after
            .handle(post_json(
                "/a/sketch/api/events",
                &json!({ "events": [stroke(Some(&ulid()))] }),
            ))
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let on_disk: Vec<Value> = log_lines(&path);
    assert_eq!(on_disk.len(), 3);
    assert_eq!(on_disk[2]["seq"], 3);
    let body = body_of(after.handle(get("/a/sketch/api/events")).await).await;
    assert_eq!(body.lines().count(), 1, "{body}");
}

/// `spec/data-api.md §4` — the schema with every declared column, `id` first, and the
/// views with their placeholders; the node facts with no application data.
#[tokio::test]
async fn test_api_schema_and_node() {
    let root = tempfile::tempdir().unwrap();
    let handler = with_meds(&root);
    let (status, schema) = json_of(handler.handle(get("/a/meds/api/schema")).await).await;
    assert_eq!(status, StatusCode::OK, "{schema}");
    let fill = &schema["tables"][0];
    assert_eq!(fill["name"], "fill");
    let columns = fill["columns"].as_array().unwrap();
    assert_eq!(
        columns[0],
        json!({ "name": "id", "type": "VARCHAR", "not_null": true })
    );
    assert_eq!(
        columns[1],
        json!({ "name": "drug", "type": "VARCHAR", "not_null": true })
    );
    assert_eq!(
        columns[2],
        json!({ "name": "copay_amount", "type": "DECIMAL(18,2)", "not_null": false })
    );
    let views: Vec<(&str, Vec<&str>)> = schema["views"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| {
            (
                v["name"].as_str().unwrap(),
                v["params"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|p| p.as_str().unwrap())
                    .collect(),
            )
        })
        .collect();
    assert_eq!(
        views,
        vec![
            ("v_fill", vec![]),
            ("v_since", vec!["since"]),
            ("v_stats", vec![])
        ]
    );
    assert_eq!(schema["schema_hash"].as_str().unwrap().len(), 64);
    let (_, empty) = json_of(handler.handle(get("/a/sketch/api/schema")).await).await;
    assert_eq!(empty["tables"], json!([]));
    assert_eq!(empty["views"], json!([]));

    let id = node_id(&handler);
    let (status, node) = json_of(handler.handle(get("/a/meds/api/node")).await).await;
    assert_eq!(status, StatusCode::OK, "{node}");
    assert_eq!(node["id"], id);
    assert_eq!(node["dev"], id);
    assert_eq!(
        node["name"], id,
        "no display name yet: the Node ID stands in"
    );
    assert_eq!(node["solo"], false);
    assert_eq!(node["peers"], 0);
    assert!(node["restore_tier"].is_number(), "{node}");
    let text = node.to_string();
    for forbidden in ["drug", "count", "fill", "profile"] {
        assert!(!text.contains(forbidden), "{forbidden} in {text}");
    }
}

/// `spec/data-api.md §7` — `api.sql_rate` per session, 429 with `Retry-After` past it.
#[tokio::test]
async fn test_api_sql_rate_limited() {
    let root = tempfile::tempdir().unwrap();
    let handler = with_meds(&root);
    set_setting(&handler, "api.sql_rate", "2");
    let query = || post_json("/a/meds/api/sql", &json!({ "sql": "SELECT 1" }));
    assert_eq!(handler.handle(query()).await.status(), StatusCode::OK);
    assert_eq!(handler.handle(query()).await.status(), StatusCode::OK);
    let third = handler.handle(query()).await;
    assert_eq!(third.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(header(&third, &RETRY_AFTER), "1");
    let (_, body) = json_of(third).await;
    assert!(
        body["error"].as_str().unwrap().contains("api.sql_rate"),
        "{body}"
    );
    // A refused request wrote nothing and read nothing; the bucket refills.
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(handler.handle(query()).await.status(), StatusCode::OK);
}

/// `spec/data-dictionary.md §4`, `docs/plans/phase-1.md §2.7` — `cache/_sys.sqlite` is
/// attached read-only as `sys` on the app's connection: its views answer to `/api/sql`
/// and to `pv.query`; nothing in it can be written or detached.
#[tokio::test]
async fn test_sys_views_readable_from_app_sql() {
    let root = tempfile::tempdir().unwrap();
    let apps = privatium_core::Paths::rooted(root.path()).apps_dir();
    fs::write(
        root.path().join("config.toml"),
        "[lua]\npool_size = 1\nmax_instructions = 5000000\nmax_memory_mb = 16\nmax_seconds = 20\n",
    )
    .unwrap();
    write_app(
        &apps,
        "peek",
        Some(&lua_manifest("peek")),
        &[(
            "app.lua",
            "local pv = require 'privatium'\n\
             pv.get('/nav', function() return pv.json(pv.query('SELECT id FROM sys.v_app_nav ORDER BY id')) end)\n\
             pv.get('/write', function()\n\
               local ok, err = pcall(pv.query, \"INSERT INTO sys.sys_setting (id, value) VALUES ('x', '1')\")\n\
               local ok2, err2 = pcall(pv.query, 'DETACH sys')\n\
               return pv.json({ write = ok, detach = ok2 })\n\
             end)\n",
        )],
    );
    let handler = with_meds(&root);
    let (status, body) = json_of(handler.handle(get("/a/peek/nav")).await).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let slugs: Vec<&str> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert!(slugs.contains(&"meds"), "{slugs:?}");
    assert!(slugs.contains(&"sketch"), "{slugs:?}");
    let (_, body) = json_of(handler.handle(get("/a/peek/write")).await).await;
    assert_eq!(body, json!({ "write": false, "detach": false }));

    let (status, body) = json_of(
        handler
            .handle(post_json(
                "/a/meds/api/sql",
                &json!({ "sql": "SELECT kind FROM sys.v_device_active" }),
            ))
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"], json!([{ "kind": "node" }]));
    let (status, body) = json_of(
        handler
            .handle(post_json(
                "/a/meds/api/sql",
                &json!({ "sql": "SELECT app_id, restore_tier FROM sys.v_health WHERE app_id = 'meds'" }),
            ))
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"][0]["app_id"], "meds");
}

// ---------------------------------------------------------------------------------------
// §3 — the stream
// ---------------------------------------------------------------------------------------

/// `spec/data-api.md §3` — `after=` resumes with no gap: the events past it come first,
/// byte-identical to the log, then live ones as they land; a reconnect from the last
/// `lam` seen repeats nothing and misses nothing.
#[tokio::test]
async fn test_spec_data_3_stream_no_gap_on_reconnect() {
    let root = tempfile::tempdir().unwrap();
    let handler = handler(&root);
    let append = |id: String| {
        post_json(
            "/a/sketch/api/events",
            &json!({ "events": [stroke(Some(&id))] }),
        )
    };
    for _ in 0..3 {
        assert_eq!(
            handler.handle(append(ulid())).await.status(),
            StatusCode::OK
        );
    }
    let disk = raw_lines(&handler, "sketch");

    let response = handler.handle(get("/a/sketch/api/stream?after=1")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(header(&response, &CONTENT_TYPE), "text/event-stream");
    assert_eq!(
        header(&response, &axum::http::header::CACHE_CONTROL),
        "no-store"
    );
    let mut body = response.into_body();
    let (event, data) = next_frame(&mut body, 5).await.unwrap();
    assert_eq!(
        (event.as_str(), data.as_str()),
        ("append", disk[1].as_str())
    );
    let (event, data) = next_frame(&mut body, 5).await.unwrap();
    assert_eq!(
        (event.as_str(), data.as_str()),
        ("append", disk[2].as_str())
    );
    assert!(next_frame(&mut body, 1).await.is_none(), "nothing else yet");

    assert_eq!(
        handler.handle(append(ulid())).await.status(),
        StatusCode::OK
    );
    let (event, data) = next_frame(&mut body, 5).await.unwrap();
    assert_eq!(event, "append");
    let fourth: Value = serde_json::from_str(&data).unwrap();
    assert_eq!(fourth["lam"], 4);
    assert_eq!(data, raw_lines(&handler, "sketch")[3]);
    drop(body);

    // Reconnect from the last lam seen: only what landed since.
    assert_eq!(
        handler.handle(append(ulid())).await.status(),
        StatusCode::OK
    );
    let response = handler.handle(get("/a/sketch/api/stream?after=4")).await;
    let mut body = response.into_body();
    let (event, data) = next_frame(&mut body, 5).await.unwrap();
    assert_eq!(event, "append");
    let fifth: Value = serde_json::from_str(&data).unwrap();
    assert_eq!(fifth["lam"], 5);
    assert!(next_frame(&mut body, 1).await.is_none());
    drop(body);

    // From the beginning, everything; a bad `after` is refused; HEAD is headers only.
    let mut body = handler
        .handle(get("/a/sketch/api/stream?after=0"))
        .await
        .into_body();
    for expected in raw_lines(&handler, "sketch") {
        let (_, data) = next_frame(&mut body, 5).await.unwrap();
        assert_eq!(data, expected);
    }
    drop(body);
    let response = handler.handle(get("/a/sketch/api/stream?after=x")).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let head = handler
        .handle(request(Method::HEAD, "/a/sketch/api/stream"))
        .await;
    assert_eq!(head.status(), StatusCode::OK);
    assert_eq!(header(&head, &CONTENT_TYPE), "text/event-stream");
    assert!(body_of(head).await.is_empty());
}

/// ADR 0003, R6 — the stream is the streaming `Response` body: `handle` returns while
/// the stream is open, and each event arrives as its own frame as it lands, not after
/// the handler finishes and never through the Lua host.
#[tokio::test]
async fn test_stream_frames_arrive_before_the_handler_finishes() {
    let root = tempfile::tempdir().unwrap();
    let handler = handler(&root);
    let response = handler.handle(get("/a/sketch/api/stream")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body();
    assert!(
        next_frame(&mut body, 1).await.is_none(),
        "an idle stream sends nothing"
    );
    for n in 1..=3 {
        let id = ulid();
        assert_eq!(
            handler
                .handle(post_json(
                    "/a/sketch/api/events",
                    &json!({ "events": [stroke(Some(&id))] })
                ))
                .await
                .status(),
            StatusCode::OK
        );
        let (event, data) = next_frame(&mut body, 5).await.unwrap();
        assert_eq!(event, "append");
        let ev: Value = serde_json::from_str(&data).unwrap();
        assert_eq!(ev["lam"], n);
        assert_eq!(ev["id"], id);
        assert_eq!(ev["d"]["color"], "#00274C");
    }
    assert!(next_frame(&mut body, 1).await.is_none());
}

/// `docs/plans/phase-1.md §8`, R5 — a cache rebuilt underneath a reader is a `resync`:
/// an edited `schema.sql` (M8's reload) and a line appended by hand both do it, on the
/// next request beneath the mount.
#[tokio::test]
async fn test_stream_resync_on_schema_change() {
    let root = tempfile::tempdir().unwrap();
    let apps = privatium_core::Paths::rooted(root.path()).apps_dir();
    let dir = write_web_app(
        &apps,
        "notes",
        &[(
            "schema.sql",
            "CREATE TABLE note (id VARCHAR PRIMARY KEY, a VARCHAR);\n",
        )],
    );
    let handler = handler(&root);
    let mut body = handler.handle(get("/a/notes/api/stream")).await.into_body();
    assert!(next_frame(&mut body, 1).await.is_none());

    fs::write(
        dir.join("schema.sql"),
        "CREATE TABLE note (id VARCHAR PRIMARY KEY, a VARCHAR, b VARCHAR);\n",
    )
    .unwrap();
    assert_eq!(
        handler.handle(get("/a/notes/api/node")).await.status(),
        StatusCode::OK
    );
    let (kind, data) = next_frame(&mut body, 5).await.unwrap();
    assert_eq!(kind, "resync");
    let resync: Value = serde_json::from_str(&data).unwrap();
    assert_eq!(resync["reason"], "rematerialized");
    assert_eq!(resync["lam"], 0);
    let (_, schema) = json_of(handler.handle(get("/a/notes/api/schema")).await).await;
    assert_eq!(schema["tables"][0]["columns"].as_array().unwrap().len(), 3);

    let dev = node_id(&handler);
    hand_append(
        &log_path(&handler, "notes"),
        &event(
            1,
            1,
            &ts_offset_secs(-1),
            &dev,
            "note",
            "a",
            Some(r#"{"a":"by hand"}"#),
        )
        .replace("\"app\":\"hello\"", "\"app\":\"notes\""),
        "\n",
    );
    assert_eq!(
        handler.handle(get("/a/notes/api/schema")).await.status(),
        StatusCode::OK
    );
    let (kind, data) = next_frame(&mut body, 5).await.unwrap();
    assert_eq!(kind, "resync");
    let resync: Value = serde_json::from_str(&data).unwrap();
    assert_eq!(resync["lam"], 1);
    let (status, body_row) = json_of(handler.handle(get("/a/notes/api/row/note/a")).await).await;
    assert_eq!(status, StatusCode::OK, "{body_row}");
    assert_eq!(body_row["d"]["a"], "by hand");
}

/// `spec/data-api.md §3` — a `ping` every interval carries the high-water mark, and its
/// stat of the app is what notices a log that grew on an idle node, so the resync follows
/// without any other request.
#[tokio::test]
async fn test_stream_ping_notices_a_log_that_grew() {
    let root = tempfile::tempdir().unwrap();
    let mut handler = handler(&root);
    handler.api_mut().set_ping(Duration::from_millis(200));
    let mut body = handler
        .handle(get("/a/sketch/api/stream"))
        .await
        .into_body();
    let (kind, data) = next_frame(&mut body, 5).await.unwrap();
    assert_eq!(kind, "ping");
    assert_eq!(serde_json::from_str::<Value>(&data).unwrap()["lam"], 0);

    let dev = node_id(&handler);
    hand_append(
        &log_path(&handler, "sketch"),
        &event(
            1,
            1,
            &ts_offset_secs(-1),
            &dev,
            "stroke",
            "byhand",
            Some("{}"),
        )
        .replace("\"app\":\"hello\"", "\"app\":\"sketch\""),
        "\n",
    );
    let mut seen = Vec::new();
    for _ in 0..4 {
        if let Some(frame) = next_frame(&mut body, 5).await {
            seen.push(frame);
        }
    }
    assert!(
        seen.iter()
            .any(|(event, data)| event == "resync" && data.contains("\"lam\":1")),
        "{seen:?}"
    );
    assert!(seen.iter().any(|(event, _)| event == "ping"), "{seen:?}");
}

/// `spec/protocol.md §4.1`, `§4.3` — a line appended by hand while the node runs
/// (`apps/hello/README.md`'s `echo >>`) moves this node's counters: the next event it
/// writes takes the next `seq` in its own file and a `lam` past the line's, never a
/// duplicate of either.
#[tokio::test]
async fn test_hand_appended_line_moves_the_counters() {
    let root = tempfile::tempdir().unwrap();
    let handler = handler(&root);
    let dev = node_id(&handler);
    hand_append(
        &log_path(&handler, "sketch"),
        &event(
            7,
            40,
            &ts_offset_secs(-1),
            &dev,
            "stroke",
            "byhand",
            Some("{}"),
        )
        .replace("\"app\":\"hello\"", "\"app\":\"sketch\""),
        "\n",
    );
    let (status, body) = json_of(
        handler
            .handle(post_json(
                "/a/sketch/api/events",
                &json!({ "events": [stroke(None)] }),
            ))
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["lam"], 41);
    let lines = log_lines(&log_path(&handler, "sketch"));
    assert_eq!(lines.len(), 2);
    assert_eq!(
        (lines[1]["seq"].as_u64(), lines[1]["lam"].as_u64()),
        (Some(8), Some(41))
    );
    let since = body_of(handler.handle(get("/a/sketch/api/events?after=40")).await).await;
    assert!(since.contains("\"seq\":8"), "{since}");
    assert_eq!(since.lines().count(), 1, "{since}");
}

/// `spec/data-api.md §7` — `api.max_streams` per device; a closed stream frees its slot.
#[tokio::test]
async fn test_api_max_streams() {
    let root = tempfile::tempdir().unwrap();
    let handler = handler(&root);
    set_setting(&handler, "api.max_streams", "1");
    let first = handler.handle(get("/a/sketch/api/stream")).await;
    assert_eq!(first.status(), StatusCode::OK);
    let (status, body) = json_of(handler.handle(get("/a/sketch/api/stream")).await).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("api.max_streams"),
        "{body}"
    );
    drop(first);
    let mut freed = false;
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if handler.handle(get("/a/sketch/api/stream")).await.status() == StatusCode::OK {
            freed = true;
            break;
        }
    }
    assert!(freed, "the slot was not freed after the client went away");
}

// ---------------------------------------------------------------------------------------
// The namespace, the fences, and sketch
// ---------------------------------------------------------------------------------------

/// `spec/protocol.md §9.1` — beneath every mount `api/` is the framework's: a Tier 2 file
/// at `web/api/…` is shadowed, a Tier 1 app answers the API beside its routes, and in solo
/// mode the API is at `/api/…` with `/api/v1/*` still the framework's.
#[tokio::test]
async fn test_api_reserved_beneath_every_mount() {
    let root = tempfile::tempdir().unwrap();
    let apps = privatium_core::Paths::rooted(root.path()).apps_dir();
    write_web_app(
        &apps,
        "shadow",
        &[("web/api/x.txt", "should never be served")],
    );
    let host = handler(&root);
    let (status, body) = json_of(host.handle(get("/a/shadow/api/x.txt")).await).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(
        body["error"].as_str().unwrap().contains("spec/data-api.md"),
        "{body}"
    );
    let (status, body) = json_of(host.handle(get("/a/hello/api/node")).await).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    for path in [
        "/a/hello/api",
        "/a/hello/api/",
        "/a/sketch/api/q",
        "/a/sketch/api/row/x",
    ] {
        let response = host.handle(get(path)).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        assert_eq!(
            header(&response, &CONTENT_TYPE),
            "application/json",
            "{path}"
        );
    }
    // The wrong method says which would work.
    let response = host
        .handle(request(Method::DELETE, "/a/sketch/api/events"))
        .await;
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(header(&response, &ALLOW), "GET, HEAD, POST");
    drop(host);

    fs::write(
        root.path().join("config.toml"),
        "[node]\nmode = \"solo\"\napp = \"sketch\"\n",
    )
    .unwrap();
    let solo = handler(&root);
    let (status, body) = json_of(solo.handle(get("/api/node")).await).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["solo"], true);
    assert_eq!(
        solo.handle(get("/api/v1/health")).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        solo.handle(get("/api/v2/health")).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        solo.handle(post_json(
            "/api/events",
            &json!({ "events": [stroke(None)] })
        ))
        .await
        .status(),
        StatusCode::OK
    );
}

/// `spec/data-api.md §2` — the API is same-origin: a browser's `Sec-Fetch-Site:
/// cross-site` is refused on every route, and a POST is read only as `application/json`,
/// which no cross-origin page can send without a preflight the node never answers.
#[tokio::test]
async fn test_api_cross_site_and_content_type_fences() {
    let root = tempfile::tempdir().unwrap();
    let handler = handler(&root);
    for path in [
        "/a/sketch/api/node",
        "/a/sketch/api/events",
        "/a/sketch/api/stream",
    ] {
        let response = handler
            .handle(with_header(get(path), "sec-fetch-site", "cross-site"))
            .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{path}");
    }
    let response = handler
        .handle(with_header(
            post_json("/a/sketch/api/events", &json!({ "events": [] })),
            "sec-fetch-site",
            "cross-site",
        ))
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    for site in ["same-origin", "none"] {
        let response = handler
            .handle(with_header(
                get("/a/sketch/api/node"),
                "sec-fetch-site",
                site,
            ))
            .await;
        assert_eq!(response.status(), StatusCode::OK, "{site}");
    }
    for content_type in [
        Some("text/plain"),
        Some("application/x-www-form-urlencoded"),
        None,
    ] {
        let response = handler
            .handle(post_raw(
                "/a/sketch/api/events",
                content_type,
                "{\"events\":[]}",
            ))
            .await;
        assert_eq!(
            response.status(),
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "{content_type:?}"
        );
    }
    let response = handler
        .handle(post_raw(
            "/a/sketch/api/events",
            Some("application/json; charset=utf-8"),
            "{\"events\":[]}",
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(raw_lines(&handler, "sketch").is_empty());
}

/// Roadmap: "`sketch` (Tier 2) works with its own JavaScript and no `schema.sql`." The
/// whole app over the API: `pv.js` served, a stroke put with no validation, read back
/// from the log as the app boots, deleted, streamed; no view, no SQL, no tables.
#[tokio::test]
async fn test_sketch_works_without_schema_sql() {
    let root = tempfile::tempdir().unwrap();
    let handler = handler(&root);
    assert!(!repo_apps_dir().join("sketch").join("schema.sql").exists());

    let index = handler.handle(get("/a/sketch/")).await;
    assert_eq!(index.status(), StatusCode::OK);
    assert!(body_of(index).await.contains("app.js"));
    let app_js = body_of(handler.handle(get("/a/sketch/app.js")).await).await;
    assert!(app_js.contains("from '/static/pv.js'"), "{app_js}");
    let pv = handler.handle(get("/static/pv.js")).await;
    assert_eq!(pv.status(), StatusCode::OK);
    assert!(header(&pv, &CONTENT_TYPE).contains("javascript"));
    assert_eq!(header(&pv, &axum::http::header::CACHE_CONTROL), "no-cache");
    let pv = body_of(pv).await;
    for name in [
        "query",
        "sql",
        "get",
        "events",
        "append",
        "put",
        "del",
        "subscribe",
        "ulid",
        "node",
        "online",
        "on",
        "url",
        "PvOffline",
    ] {
        assert!(pv.contains(name), "pv.js lacks {name}");
    }
    assert!(!pv.contains("dedupe"), "no dedupe table (AGENTS.md 11)");
    for helper in [
        "pv.events",
        "pv.put",
        "pv.append",
        "pv.subscribe",
        "pv.on('resync'",
    ] {
        assert!(app_js.contains(helper), "sketch does not use {helper}");
    }

    let mut stream = handler
        .handle(get("/a/sketch/api/stream"))
        .await
        .into_body();
    let id = ulid();
    let put = json!({ "events": [{ "op": "put", "tbl": "stroke", "id": id,
        "d": { "points": [[0, 0], [4, 9]], "color": "#FFCB05", "width": 3, "anything": { "goes": true } } }] });
    let (status, body) = json_of(
        handler
            .handle(post_json("/a/sketch/api/events", &put))
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["ids"][0], id);
    let (event, data) = next_frame(&mut stream, 5).await.unwrap();
    assert_eq!(event, "append");
    let disk = raw_lines(&handler, "sketch");
    assert_eq!(data, disk[0]);

    // What the app reads at boot: the log, as lines.
    let boot = body_of(handler.handle(get("/a/sketch/api/events?tbl=stroke")).await).await;
    assert_eq!(boot, format!("{}\n", disk[0]));
    let ev: Value = serde_json::from_str(disk[0].as_str()).unwrap();
    assert_eq!(ev["d"]["anything"]["goes"], true, "stored as-is, no schema");
    let (status, row) = json_of(
        handler
            .handle(get(&format!("/a/sketch/api/row/stroke/{id}")))
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(row["d"]["color"], "#FFCB05");

    let (status, _) = json_of(
        handler
            .handle(post_json(
                "/a/sketch/api/events",
                &json!({ "events": [{ "op": "del", "tbl": "stroke", "id": id }] }),
            ))
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (event, data) = next_frame(&mut stream, 5).await.unwrap();
    assert_eq!(event, "append");
    assert!(data.contains("\"op\":\"del\""), "{data}");
    let response = handler
        .handle(get(&format!("/a/sketch/api/row/stroke/{id}")))
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    // No tables, no views, no SQL — and the app does not need them.
    assert_eq!(
        handler
            .handle(get("/a/sketch/api/q/v_anything"))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        handler
            .handle(post_json(
                "/a/sketch/api/sql",
                &json!({ "sql": "SELECT 1" })
            ))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let (_, schema) = json_of(handler.handle(get("/a/sketch/api/schema")).await).await;
    assert_eq!(schema["tables"], json!([]));
}
