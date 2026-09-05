// Project:  Privatium™  |  File: crates/privatium-core/tests/embedded.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-05  |  Modified: 2026-09-05
// Summary:  spec/app-contract.md §2.3 and §6 against the crate as a library (M13): a node
//           opened with no app folders, an app this binary owns with its schema inline,
//           append, append_batch, query with bound parameters and the data API's typing,
//           subscribe, snapshot and restore, close; the four Phase 2 and 3 methods that
//           are present and never Ok; auth_layer around an embedder's own axum router;
//           the sandbox holding under query (§7); and the example being what the spec
//           shows, in thirty lines.

// AGENTS.md, Style: unwrap() is permitted in tests, and a test that hides a failure
// behind `?` is worse than one that panics with a line number.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::net::SocketAddr;
use std::path::Path;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::{ConnectInfo, Request};
use axum::http::{StatusCode, header::HOST};
use axum::routing::get;
use privatium_core::app::StreamEvent;
use privatium_core::{AppRoot, Device, Error, Event, Node, Peer, new_ulid};
use serde_json::{Value, json};
use tower::ServiceExt as _;

/// The embedder's own app: no folder anywhere, its schema a string in the binary.
const APP: &str = "scores";

const SCHEMA: &str = "CREATE TABLE score (
    id     VARCHAR PRIMARY KEY,
    player VARCHAR NOT NULL,
    points BIGINT,
    best   BOOLEAN
);";

/// `Node::open` then `open_app`, which is every start of an embedded program.
fn open(root: &Path) -> Node {
    let mut node = Node::open(root).unwrap();
    node.open_app(APP, SCHEMA).unwrap();
    node
}

fn log_lines(root: &Path, dev: &str) -> Vec<String> {
    let path = root
        .join("data")
        .join(APP)
        .join("log")
        .join(format!("{dev}.jsonl"));
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// `§2.3` — the shape the spec shows, with no folder under `apps/`: open, open the app
/// with its schema, append one event, append a batch, query with a bound parameter and
/// read the rows typed as the data API types them (`spec/data-api.md §1`), close.
#[test]
fn test_spec_app_contract_2_3_open_app_append_query_with_no_folder() {
    let root = tempfile::tempdir().unwrap();
    let mut node = open(root.path());
    let dev = node.id().as_str().to_owned();

    // No folder was scanned or created; the app exists all the same.
    assert_eq!(fs::read_dir(node.paths().apps_dir()).unwrap().count(), 0);
    let app = node.app(APP).unwrap();
    assert_eq!(app.slug(), APP);
    assert!(app.dir().is_none(), "an embedded app has no folder");
    assert!(app.mount().is_none(), "and is not served by the shell");
    assert_eq!(
        app.manifest().app.tier,
        privatium_core::app::Tier::Rust,
        "a Tier 3 app (spec/app-contract.md §6)"
    );

    let ada = new_ulid();
    let first = node
        .append(
            APP,
            Event::put(
                "score",
                &ada,
                json!({ "player": "ada", "points": 42, "best": true }),
            ),
        )
        .unwrap();
    assert_eq!(first.app, APP);
    assert_eq!(first.seq, 1);
    assert_eq!(first.dev, dev);
    assert_eq!(first.events.len(), 1);
    assert_eq!(first.events[0].tbl, "score");
    assert_eq!(first.events[0].id, ada);

    let batch = node
        .append_batch(
            APP,
            vec![
                Event::put(
                    "score",
                    new_ulid(),
                    json!({ "player": "bob", "points": "7", "best": false }),
                ),
                Event::del("score", &ada),
            ],
        )
        .unwrap();
    assert_eq!(batch.seq, 2);
    assert_eq!(batch.lam, first.lam + 1);
    assert_eq!(batch.events.len(), 2);
    assert_eq!(batch.lines.len(), 2);
    assert!(batch.events[1].d.is_none(), "a del carries no d");

    let rows = node
        .query(
            APP,
            "SELECT player, points, best FROM score WHERE points > ? ORDER BY points DESC",
            &[json!(0)],
        )
        .unwrap();
    assert_eq!(rows.len(), 1, "ada was deleted: {rows:?}");
    assert_eq!(rows[0]["player"], json!("bob"));
    assert_eq!(rows[0]["points"], json!("7"), "BIGINT arrives as a string");
    assert_eq!(
        rows[0]["best"],
        json!(false),
        "BOOLEAN arrives as a boolean"
    );
    let none = node
        .query(
            APP,
            "SELECT * FROM score WHERE player = ?",
            &[json!("nobody")],
        )
        .unwrap();
    assert!(none.is_empty());

    // The log is the spec's: this node's own file under data/<slug>/log/, one line per
    // event, the batch marked on its first line (spec/protocol.md §4.1).
    let lines = log_lines(root.path(), &dev);
    assert_eq!(lines.len(), 3);
    assert!(lines[0].contains("\"app\":\"scores\""), "{}", lines[0]);
    assert!(lines[0].contains("\"tbl\":\"score\""), "{}", lines[0]);
    assert!(lines[1].contains("\"batch\":2"), "{}", lines[1]);
    assert!(lines[2].contains("\"op\":\"del\""), "{}", lines[2]);
    assert_eq!(lines[1].as_bytes(), batch.lines[0].as_slice());

    node.close().unwrap();
    let state = fs::read_to_string(root.path().join("local").join("state.jsonl")).unwrap();
    assert!(
        state.contains(APP),
        "close flushed local/state.jsonl: {state}"
    );
}

/// `§2.3` — a second start opens the same app again: the rows are there, `seq` continues
/// from the file, and with `cache/` deleted the tables come back from the log
/// (`spec/protocol.md §3.1`, `§5.3`).
#[test]
fn test_spec_app_contract_2_3_embedded_app_survives_restart_and_a_cache_delete() {
    let root = tempfile::tempdir().unwrap();
    {
        let mut node = open(root.path());
        node.append(
            APP,
            Event::put("score", new_ulid(), json!({ "player": "ada", "points": 1 })),
        )
        .unwrap();
        node.close().unwrap();
    }
    {
        let mut node = open(root.path());
        let rows = node.query(APP, "SELECT player FROM score", &[]).unwrap();
        assert_eq!(rows.len(), 1);
        let again = node
            .append(
                APP,
                Event::put("score", new_ulid(), json!({ "player": "bob", "points": 2 })),
            )
            .unwrap();
        assert_eq!(again.seq, 2, "seq continues from the file");
        node.close().unwrap();
    }
    fs::remove_dir_all(root.path().join("cache")).unwrap();
    let node = open(root.path());
    let rows = node
        .query(APP, "SELECT player FROM score ORDER BY points", &[])
        .unwrap();
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(rows[1]["player"], json!("bob"));
    assert!(
        node.restore_tier(APP).is_some(),
        "the tier that rebuilt the cache is recorded (spec/protocol.md §5.3)"
    );
}

/// `§2.3`, `spec/protocol.md §1.1`, `spec/app-contract.md §3.1` — `open_app` refuses a
/// reserved or malformed slug and a slug a folder already holds; an app never opened is
/// `AppNotLoaded` rather than a silent success; opening the same embedded app twice is a
/// reopen, as a folder's reload is.
#[test]
fn test_spec_app_contract_2_3_open_app_refuses_bad_slugs_and_a_folder_collision() {
    let root = tempfile::tempdir().unwrap();
    let mut node = Node::open(root.path()).unwrap();

    for slug in [
        "_sys",
        "api",
        "a",
        "settings",
        "Bad",
        "x",
        "has space",
        "1st",
    ] {
        let refused = node.open_app(slug, "").unwrap_err();
        assert!(
            matches!(refused, Error::AppRefused { .. }),
            "{slug}: {refused}"
        );
    }
    assert!(matches!(
        node.append(APP, Event::del("score", "x")),
        Err(Error::AppNotLoaded { .. })
    ));
    assert!(matches!(
        node.query(APP, "SELECT 1", &[]),
        Err(Error::AppNotLoaded { .. })
    ));
    assert!(matches!(
        node.subscribe(APP),
        Err(Error::AppNotLoaded { .. })
    ));

    // A folder app of the same slug was loaded first: the embedder is refused, loudly.
    let apps = node.paths().apps_dir();
    let folder = apps.join("box");
    fs::create_dir_all(folder.join("web")).unwrap();
    fs::write(
        folder.join("app.toml"),
        "[app]\nslug = \"box\"\ntitle = \"Box\"\nversion = \"1.0.0\"\napi = 1\ntier = \"web\"\n",
    )
    .unwrap();
    fs::write(folder.join("web").join("index.html"), "<p>box</p>").unwrap();
    let report = node.load_apps(&[AppRoot::local(apps)]).unwrap();
    assert_eq!(report.loaded, vec!["box".to_owned()]);
    let refused = node.open_app("box", "").unwrap_err();
    assert!(
        matches!(&refused, Error::AppRefused { reason, .. } if reason.contains("folder")),
        "{refused}"
    );

    // The same embedded app twice: a reopen, and the data is still one app's.
    node.open_app(APP, SCHEMA).unwrap();
    node.append(
        APP,
        Event::put("score", new_ulid(), json!({ "player": "ada", "points": 1 })),
    )
    .unwrap();
    node.open_app(APP, SCHEMA).unwrap();
    assert_eq!(
        node.query(APP, "SELECT count(*) AS n FROM score", &[])
            .unwrap()[0]["n"],
        json!(1)
    );
    // And the folder app is untouched by any of it.
    assert!(node.app("box").unwrap().dir().is_some());
}

/// `§6` `subscribe` — every event this node appends to the app reaches a subscriber as
/// its log line, in order, with its `lam` (`spec/data-api.md §3` is one such
/// subscriber; an embedder is another).
#[test]
fn test_spec_app_contract_6_subscribe_sees_each_append() {
    let root = tempfile::tempdir().unwrap();
    let mut node = open(root.path());
    let mut stream = node.subscribe(APP).unwrap();
    let appended = node
        .append_batch(
            APP,
            vec![
                Event::put("score", new_ulid(), json!({ "player": "ada", "points": 1 })),
                Event::put("score", new_ulid(), json!({ "player": "bob", "points": 2 })),
            ],
        )
        .unwrap();
    for offset in 0..2u64 {
        match stream.try_recv().unwrap() {
            StreamEvent::Append { lam, line } => {
                assert_eq!(lam, appended.lam + offset);
                assert_eq!(line.as_ref(), appended.lines[offset as usize].as_slice());
            }
            other => panic!("expected an append, got {other:?}"),
        }
    }
    assert!(stream.try_recv().is_err(), "nothing else was sent");
}

/// `§6` `snapshot` / `restore` — the node-level maintenance reaches an embedded app the
/// way it reaches a folder's: a snapshot under `data/<slug>/snap/`, and a restore that
/// reports its tier and leaves the rows readable.
#[test]
fn test_spec_app_contract_6_snapshot_and_restore_reach_an_embedded_app() {
    let root = tempfile::tempdir().unwrap();
    let mut node = open(root.path());
    node.append(
        APP,
        Event::put("score", new_ulid(), json!({ "player": "ada", "points": 3 })),
    )
    .unwrap();
    let snapshot = node.snapshot(APP).unwrap();
    assert!(
        root.path()
            .join("data")
            .join(APP)
            .join("snap")
            .join(snapshot.id.to_string())
            .is_dir()
    );
    let restored = node.restore(APP).unwrap();
    assert_eq!(
        restored.tier,
        privatium_core::Tier::Sqlite,
        "a snapshot was just written, so tier 1 rebuilds from it: {restored:?}"
    );
    let rows = node.query(APP, "SELECT points FROM score", &[]).unwrap();
    assert_eq!(rows[0]["points"], json!("3"));
}

/// `§6` — `serve_discovery`, `pair`, `start_sync` and `sync_now` are present with their
/// signatures and answer with a typed error naming the phase they arrive in. Never
/// `Ok`: a no-op that succeeded is what an embedder would build on
/// (`docs/plans/phase-1.md`, M13).
#[test]
fn test_spec_app_contract_6_phase_2_methods_never_ok() {
    let root = tempfile::tempdir().unwrap();
    let mut node = open(root.path());
    let outcomes: [(&str, privatium_core::Result<()>); 4] = [
        ("serve_discovery", node.serve_discovery()),
        ("pair", node.pair()),
        ("start_sync", node.start_sync()),
        ("sync_now", node.sync_now()),
    ];
    for (name, outcome) in outcomes {
        let error = outcome.expect_err(name);
        match &error {
            Error::Unimplemented {
                feature,
                phase,
                spec,
            } => {
                assert_eq!(*feature, name);
                assert!(*phase == "2" || *phase == "3", "{name}: phase {phase}");
                assert!(spec.starts_with("spec/protocol.md §"), "{name}: {spec}");
            }
            other => panic!("{name}: {other}"),
        }
        let text = error.to_string();
        assert!(text.contains("not in this build"), "{text}");
        assert!(text.contains("docs/roadmap.md"), "{text}");
    }
}

/// `§6` `auth_layer` around an embedder's own router (`§2.3`): a request from loopback, as
/// `into_make_service_with_connect_info` reports it, is this node's device; one from
/// anywhere else, or with a `Host` that does not name this machine, is 403 before the
/// route runs (`docs/plans/phase-1.md §2.2`). A request whose peer the layer cannot see —
/// a router served without `into_make_service_with_connect_info` — is refused too, naming
/// the missing call: the layer fails closed, never open. An embedder calling their router
/// in-process says so by inserting `Peer` themselves.
#[tokio::test]
async fn test_spec_app_contract_6_auth_layer_wraps_an_embedders_router() {
    let root = tempfile::tempdir().unwrap();
    let node = open(root.path());
    let id = node.id().as_str().to_owned();
    let router = Router::new()
        .route(
            "/",
            get(|request: Request| async move {
                request.extensions().get::<Device>().map_or_else(
                    || "nobody".to_owned(),
                    |device| device.0.as_str().to_owned(),
                )
            }),
        )
        .layer(node.auth_layer());

    let call = |peer: Option<SocketAddr>, marked: Option<Peer>, host: Option<&str>| {
        let router = router.clone();
        let host = host.map(str::to_owned);
        async move {
            let mut request = axum::http::Request::get("/").body(Body::empty()).unwrap();
            if let Some(peer) = peer {
                request.extensions_mut().insert(ConnectInfo(peer));
            }
            if let Some(marked) = marked {
                request.extensions_mut().insert(marked);
            }
            if let Some(host) = host {
                request.headers_mut().insert(HOST, host.parse().unwrap());
            }
            let response = router.oneshot(request).await.unwrap();
            let status = response.status();
            let body = to_bytes(response.into_body(), 1 << 16).await.unwrap();
            (status, String::from_utf8_lossy(&body).into_owned())
        }
    };

    // No peer at all: the router was served without connect info. Refused, and the
    // refusal says which call is missing — an embedder who forgot it gets a router that
    // admits nobody, never one that admits everybody.
    let (status, body) = call(None, None, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(
        body.contains("into_make_service_with_connect_info"),
        "{body}"
    );
    assert!(body.contains("Peer"), "{body}");

    let loopback: SocketAddr = ([127, 0, 0, 1], 40000).into();
    let (status, body) = call(Some(loopback), None, Some("localhost:8421")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, id);

    // In-process, said so: the embedder inserted the peer the framework's adapter would.
    let (status, body) = call(None, Some(Peer(loopback)), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, id, "in-process: this node's device");

    let elsewhere: SocketAddr = ([10, 0, 0, 7], 40000).into();
    let (status, body) = call(Some(elsewhere), None, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(body.contains("loopback only"), "{body}");
    assert!(
        !body.contains(&id),
        "the refusal names nothing about the node"
    );

    let (status, _) = call(Some(loopback), None, Some("evil.example:8421")).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "DNS rebinding: the Host gives it away"
    );
}

/// `§7` — `query` runs on the sandboxed connection: a write, a `PRAGMA` and a mismatched
/// parameter count are refused, and `sys` is attached read-only
/// (`spec/data-dictionary.md §4`).
#[test]
fn test_spec_app_contract_7_query_cannot_write() {
    let root = tempfile::tempdir().unwrap();
    let node = open(root.path());
    for sql in [
        "INSERT INTO score (id, player) VALUES ('x', 'y')",
        "DELETE FROM score",
        "PRAGMA journal_mode = wal",
        "ATTACH DATABASE ':memory:' AS other",
    ] {
        let refused = node.query(APP, sql, &[]).unwrap_err();
        assert!(
            matches!(&refused, Error::Sql { app, .. } if app == APP),
            "{sql}: {refused}"
        );
    }
    let refused = node
        .query(APP, "SELECT ? AS a, ? AS b", &[json!(1)])
        .unwrap_err();
    assert!(refused.to_string().contains("placeholder"), "{refused}");
    let refused = node
        .query(APP, "SELECT ? AS a", &[json!({ "not": "a scalar" })])
        .unwrap_err();
    assert!(refused.to_string().contains("scalar"), "{refused}");

    let devices = node
        .query(APP, "SELECT id, kind FROM sys.sys_device", &[])
        .unwrap();
    assert_eq!(devices.len(), 1);
    assert_eq!(
        devices[0]["id"],
        Value::String(node.id().as_str().to_owned())
    );
    assert_eq!(devices[0]["kind"], json!("node"));
}

/// `§2.3`, `docs/plans/phase-1.md` M13 — `examples/embedded.rs` is the spec's shape in
/// thirty lines: open, the app, an append, a query, an own axum router behind
/// `auth_layer`, close. CI compiles and runs it; this holds its size and its content.
#[test]
fn test_spec_app_contract_2_3_example_is_thirty_lines_of_the_spec_shape() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("embedded.rs");
    let text = fs::read_to_string(&path).unwrap();
    let body: Vec<&str> = text
        .lines()
        .skip_while(|line| line.starts_with("//"))
        .filter(|line| !line.trim().is_empty())
        .collect();
    assert!(
        body.len() <= 30,
        "{} lines after the header; the plan says thirty:\n{}",
        body.len(),
        body.join("\n")
    );
    for needle in [
        "Node::open(",
        ".open_app(",
        ".append(",
        "Event::put(",
        ".query(",
        "Router::new()",
        ".auth_layer()",
        "axum::serve(",
        "into_make_service_with_connect_info",
        ".close()",
    ] {
        assert!(text.contains(needle), "the example lacks {needle}");
    }
    assert!(
        !text.contains("unwrap()") && !text.contains("expect("),
        "AGENTS.md, Style: no unwrap outside tests"
    );
}
