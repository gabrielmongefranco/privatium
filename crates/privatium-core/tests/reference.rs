// Project:  Privatium™  |  File: crates/privatium-core/tests/reference.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-04  |  Modified: 2026-09-05
// Summary:  docs/plans/phase-1.md M10 — the three reference apps end to end through
//           core::handle with no listener, exactly as their READMEs describe them: hello
//           (write, amend, break the cache, and the README's own `echo >>` line run for
//           real), animals (the seed, a round over htmx and over plain form posts, the
//           three-event teach, the recursive knowledge page, reset as tombstones, and the
//           CSP the Alpine build runs under), sketch (every call app.js makes, against the
//           log). Then the accessibility baseline: the PV4xx rules of spec/cli.md §5 held
//           over the shell's own pages, the Tier 1 page frame and the reference views
//           (§5.4), and the declared colour tokens at their contrast floors (PV406).

// AGENTS.md, Style: unwrap() is permitted in tests, and a test that hides a failure
// behind `?` is worse than one that panics with a line number.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::Duration;

use axum::body::{Body, HttpBody as _, to_bytes};
use axum::http::header::{CONTENT_SECURITY_POLICY, CONTENT_TYPE, LOCATION};
use axum::http::{Method, StatusCode};
use common::a11y::{self, Unit};
use common::{hand_append, log_lines, lua_manifest, repo_apps_dir, write_app, write_web_app};
use privatium_core::http::shell;
use privatium_core::{AppRoot, Handler, LoadReport, Node, Request, Response};
use serde_json::{Value, json};

/// A small VM pool, as tests/lua.rs uses.
const LUA_CONFIG: &str =
    "[lua]\npool_size = 2\nmax_instructions = 5000000\nmax_memory_mb = 16\nmax_seconds = 20\n";

fn configure(root: &tempfile::TempDir, config: &str) {
    fs::write(root.path().join("config.toml"), config).unwrap();
}

/// A node with the owner's `apps/` and the repository's reference apps as `bundled`.
fn open(root: &tempfile::TempDir) -> (Node, LoadReport) {
    let mut node = Node::open(root.path()).unwrap();
    let roots = [
        AppRoot::local(node.paths().apps_dir()),
        AppRoot::bundled(repo_apps_dir()),
    ];
    let report = node.load_apps(&roots).unwrap();
    (node, report)
}

fn handler_for(root: &tempfile::TempDir) -> Handler {
    let (node, report) = open(root);
    Handler::new(node, report)
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

fn post(path: &str, form: &str) -> Request {
    axum::http::Request::builder()
        .method(Method::POST)
        .uri(path)
        .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(form.to_owned()))
        .unwrap()
}

fn post_json(path: &str, value: &Value) -> Request {
    axum::http::Request::builder()
        .method(Method::POST)
        .uri(path)
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(value.to_string()))
        .unwrap()
}

/// The mount a host-mode path lies beneath: `/a/<slug>/`.
fn mount_of(path: &str) -> String {
    let slug = path.trim_start_matches("/a/").split('/').next().unwrap();
    format!("/a/{slug}/")
}

/// The mount's token — what `csrf()` emits (`spec/lua-api.md §4.1`).
fn token(handler: &Handler, mount: &str) -> String {
    handler.csrf().token(mount)
}

/// A form POST carrying the mount's token, as a form with `<?= csrf() ?>` would.
fn posted(handler: &Handler, path: &str, form: &str) -> Request {
    let token = token(handler, &mount_of(path));
    let body = if form.is_empty() {
        format!("_csrf={token}")
    } else {
        format!("{form}&_csrf={token}")
    };
    post(path, &body)
}

fn with_htmx(mut request: Request) -> Request {
    request
        .headers_mut()
        .insert("hx-request", "true".parse().unwrap());
    request
}

async fn body_of(response: Response) -> String {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    String::from_utf8_lossy(&bytes).into_owned()
}

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

fn log_path(handler: &Handler, slug: &str) -> PathBuf {
    let node = handler.node().lock().unwrap();
    node.paths().app_log(slug, node.id())
}

fn node_id(handler: &Handler) -> String {
    handler.node().lock().unwrap().id().as_str().to_owned()
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

/// A ULID the way `pv.js` mints one: ten characters of time, sixteen of randomness.
fn ulid() -> String {
    const ALPHABET: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let mut millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let mut out = vec![b'0'; 26];
    for slot in (0..10).rev() {
        out[slot] = ALPHABET[(millis % 32) as usize];
        millis /= 32;
    }
    let mut seed = std::ptr::addr_of!(out) as u64 ^ millis as u64;
    for slot in out.iter_mut().skip(10) {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        *slot = ALPHABET[(seed % 32) as usize];
    }
    String::from_utf8(out).unwrap()
}

fn is_ulid(text: &str) -> bool {
    text.len() == 26
        && text
            .bytes()
            .all(|b| b"0123456789ABCDEFGHJKMNPQRSTVWXYZ".contains(&b))
}

/// The `x-data` values a rendered page carries, and the `Alpine.data('…')` names an app's
/// script registers — under the CSP build the former must be a subset of the latter.
fn x_data_values(html: &str) -> BTreeSet<String> {
    a11y::parse(html)
        .descendants()
        .iter()
        .filter_map(|e| e.attr("x-data").map(str::to_owned))
        .collect()
}

fn alpine_components(script: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut rest = script;
    while let Some(at) = rest.find("Alpine.data('") {
        let after = &rest[at + "Alpine.data('".len()..];
        let end = after.find('\'').unwrap();
        out.insert(after[..end].to_owned());
        rest = &after[end..];
    }
    out
}

fn assert_clean(what: &str, html: &str, unit: Unit) {
    let findings = a11y::check(html, unit);
    assert!(
        findings.is_empty(),
        "{what}: {} finding(s):\n  {}\n\n{html}",
        findings.len(),
        findings.join("\n  ")
    );
}

// ---------------------------------------------------------------------------------------
// hello
// ---------------------------------------------------------------------------------------

/// `apps/hello/README.md` — load, the greeting inside the frame, the form with its label
/// and token, a name written as one line of the log checked field by field, an edit that
/// amends the same row, and "break the cache; nothing is lost".
#[tokio::test]
async fn test_hello_end_to_end() {
    let root = tempfile::tempdir().unwrap();
    configure(&root, LUA_CONFIG);
    let (node, report) = open(&root);
    assert!(report.loaded.contains(&"hello".to_owned()), "{report:?}");
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert_eq!(node.app("hello").unwrap().mount(), Some("/a/hello/"));
    let dev = node.id().as_str().to_owned();
    let cache_dir = node.paths().cache_dir();
    let snap_dir = node.paths().app_snap_dir("hello");
    let log = node.paths().app_log("hello", node.id());
    let handler = Handler::new(node, report);
    let mount_token = token(&handler, "/a/hello/");

    // The empty state, inside the frame, with the page's one h1 supplied by the view.
    let home = handler.handle(get("/a/hello/")).await;
    assert_eq!(home.status(), StatusCode::OK);
    assert_eq!(header(&home, &CONTENT_TYPE), "text/html; charset=utf-8");
    let text = body_of(home).await;
    assert!(text.starts_with("<!doctype html>"), "{text}");
    assert!(text.contains("<html lang=\"en\">"), "{text}");
    assert!(text.contains("<title>Hello — Privatium</title>"), "{text}");
    assert!(text.contains("<main id=\"main\">"), "{text}");
    assert!(text.contains("<h1>We haven't met yet.</h1>"), "{text}");
    assert!(text.contains("href=\"/a/hello/edit\""), "{text}");
    assert_eq!(text.matches("<h1").count(), 1, "{text}");

    // The form: one field, labelled by `for`, and csrf() in it.
    let edit = body_of(handler.handle(get("/a/hello/edit")).await).await;
    assert!(edit.contains("<label for=\"display_name\">"), "{edit}");
    assert!(edit.contains("id=\"display_name\""), "{edit}");
    assert!(
        edit.contains(&format!("name=\"_csrf\" value=\"{mount_token}\"")),
        "{edit}"
    );
    assert!(
        edit.contains("<form method=\"post\" action=\"/a/hello/name\">"),
        "{edit}"
    );

    // A name: 303 home, greeted, and one line in the log with every field as §4.1 has it.
    let named = handler
        .handle(posted(&handler, "/a/hello/name", "display_name=Ada"))
        .await;
    assert_eq!(named.status(), StatusCode::SEE_OTHER);
    assert_eq!(header(&named, &LOCATION), "/a/hello/");
    let greeting = body_of(handler.handle(get("/a/hello/")).await).await;
    assert!(greeting.contains("Good "), "{greeting}");
    assert!(greeting.contains("Ada."), "{greeting}");
    assert!(greeting.contains("Change my name"), "{greeting}");
    let lines = log_lines(&log);
    assert_eq!(lines.len(), 1);
    let line = &lines[0];
    let keys: BTreeSet<&str> = line
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        ["seq", "lam", "ts", "dev", "app", "op", "tbl", "id", "d"]
            .into_iter()
            .collect()
    );
    assert_eq!(line["seq"], 1);
    assert_eq!(line["lam"], 1);
    assert_eq!(line["dev"], dev);
    assert_eq!(line["app"], "hello");
    assert_eq!(line["op"], "put");
    assert_eq!(line["tbl"], "profile");
    assert_eq!(line["d"], json!({ "display_name": "Ada" }));
    assert!(is_ulid(line["id"].as_str().unwrap()), "{line}");
    assert!(line["ts"].as_str().unwrap().ends_with('Z'), "{line}");
    let profile_id = line["id"].as_str().unwrap().to_owned();

    // An edit reuses the id: an amendment, one row in the table, two lines in the log.
    let renamed = handler
        .handle(posted(&handler, "/a/hello/name", "display_name=Grace"))
        .await;
    assert_eq!(renamed.status(), StatusCode::SEE_OTHER);
    let lines = log_lines(&log);
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[1]["id"], profile_id);
    assert_eq!(lines[1]["seq"], 2);
    assert_eq!(lines[1]["lam"], 2);
    {
        let node = handler.node().lock().unwrap();
        let conn = node.app("hello").unwrap().store().app_conn().unwrap();
        let mut statement = conn
            .prepare("SELECT id, display_name FROM profile")
            .unwrap();
        let rows: Vec<(String, String)> = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(rows, vec![(profile_id.clone(), "Grace".to_owned())]);
    }

    // "Break the cache; nothing is lost": the cache and the snapshots gone, the node
    // reopened, the greeting still there and the next edit still an amendment.
    drop(handler);
    fs::remove_dir_all(&cache_dir).unwrap();
    let _ = fs::remove_dir_all(&snap_dir);
    let handler = handler_for(&root);
    let greeting = body_of(handler.handle(get("/a/hello/")).await).await;
    assert!(greeting.contains("Grace."), "{greeting}");
    let again = handler
        .handle(posted(&handler, "/a/hello/name", "display_name=Linus"))
        .await;
    assert_eq!(again.status(), StatusCode::SEE_OTHER);
    let lines = log_lines(&log);
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[2]["id"], profile_id);
    assert_eq!(lines[2]["seq"], 3);
    assert_eq!(lines[2]["lam"], 3);
}

/// `apps/hello/README.md`, "Try this" — the README's own `echo '…' >> …` command, parsed
/// out of the file and run against a log that holds the three lines it describes: the
/// path is this device's log, `seq` 4 keeps the file gapless (`spec/protocol.md §4.1`),
/// `lam` 4 beats the row it amends (`§4.5`), the page shows it, and the next write the
/// node makes takes `seq` 5 and `lam` 5 (`§4.3`, M9's rescan).
#[tokio::test]
async fn test_hello_readme_echo_example_is_valid() {
    let root = tempfile::tempdir().unwrap();
    configure(&root, LUA_CONFIG);
    let handler = handler_for(&root);
    let dev = node_id(&handler);
    let log = log_path(&handler, "hello");
    for name in ["Ada", "Grace", "Linus"] {
        let response = handler
            .handle(posted(
                &handler,
                "/a/hello/name",
                &format!("display_name={name}"),
            ))
            .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
    }
    let lines = log_lines(&log);
    assert_eq!(
        lines.len(),
        3,
        "the README says three name changes are three lines"
    );
    let profile_id = lines[0]["id"].as_str().unwrap().to_owned();
    assert_eq!(lines[2]["lam"], 3);

    // The command, as written: join the `\` continuation, take the single-quoted JSON
    // and the `>>` path.
    let readme = fs::read_to_string(repo_apps_dir().join("hello").join("README.md")).unwrap();
    let mut joined = String::new();
    for line in readme.lines() {
        match line.strip_suffix('\\') {
            Some(head) => joined.push_str(head),
            None => {
                joined.push_str(line);
                joined.push('\n');
            }
        }
    }
    let command = joined
        .lines()
        .find(|line| line.trim_start().starts_with("echo '"))
        .expect("the README's echo example")
        .trim();
    let quoted_start = command.find('\'').unwrap() + 1;
    let quoted_end = command.rfind('\'').unwrap();
    let json_template = &command[quoted_start..quoted_end];
    let path_template = command[quoted_end + 1..]
        .trim()
        .strip_prefix(">>")
        .expect("appends with >>")
        .trim();
    assert_eq!(path_template, "data/hello/log/<your-id>.jsonl");
    let path = path_template.replace("<your-id>", &dev);
    assert_eq!(
        root.path().join(&path),
        log,
        "the README's path is this device's log"
    );
    let line = json_template
        .replace("<your-id>", &dev)
        .replace("<the-ulid>", &profile_id);
    let value: Value = serde_json::from_str(&line).expect("the README's JSON parses");
    assert_eq!(value["seq"], 4, "the next seq after three lines");
    assert_eq!(
        value["lam"], 4,
        "above the log's highest lam, or it loses the merge"
    );
    assert_eq!(value["dev"], dev);
    assert_eq!(value["app"], "hello");
    assert_eq!(value["op"], "put");
    assert_eq!(value["tbl"], "profile");
    assert_eq!(value["id"], profile_id);
    assert_eq!(value["d"]["display_name"], "Someone Else");

    hand_append(&log, &line, "\n");
    let lines = log_lines(&log);
    let seqs: Vec<u64> = lines.iter().map(|l| l["seq"].as_u64().unwrap()).collect();
    assert_eq!(seqs, [1, 2, 3, 4], "gapless");

    // Reload the page: the hand-written line won.
    let greeting = body_of(handler.handle(get("/a/hello/")).await).await;
    assert!(greeting.contains("Someone Else."), "{greeting}");

    // The node picked both counters up from the file: the next write is seq 5, lam 5,
    // and still an amendment of the same row.
    let fifth = handler
        .handle(posted(&handler, "/a/hello/name", "display_name=Fifth"))
        .await;
    assert_eq!(fifth.status(), StatusCode::SEE_OTHER);
    let lines = log_lines(&log);
    assert_eq!(lines.len(), 5);
    assert_eq!(lines[4]["seq"], 5);
    assert_eq!(lines[4]["lam"], 5);
    assert_eq!(lines[4]["id"], profile_id);
    let greeting = body_of(handler.handle(get("/a/hello/")).await).await;
    assert!(greeting.contains("Fifth."), "{greeting}");
}

// ---------------------------------------------------------------------------------------
// animals
// ---------------------------------------------------------------------------------------

/// The seed's events as the file spells them, in order.
fn animals_seed() -> Vec<Value> {
    fs::read_to_string(
        repo_apps_dir()
            .join("animals")
            .join("sample")
            .join("seed.jsonl"),
    )
    .unwrap()
    .lines()
    .map(|line| serde_json::from_str(line).unwrap())
    .collect()
}

/// Load `sample/seed.jsonl` through the settings page, as the owner does.
async fn seed_animals(handler: &Handler) {
    let action = "/settings/apps/animals/seed";
    let page = body_of(handler.handle(get("/settings/apps")).await).await;
    assert!(page.contains(&format!("action=\"{action}\"")), "{page}");
    let token = handler.csrf().token(action);
    let seeded = handler
        .handle(post(action, &format!("_csrf={token}")))
        .await;
    assert_eq!(seeded.status(), StatusCode::SEE_OTHER);
}

/// `apps/animals/README.md` — the seed through the settings page as this node's events,
/// a round over htmx (the `_board` fragment, the cursor row, the same question on a
/// reload), the three-event teach that turns the leaf into the question in one batch, the
/// recursive knowledge page, reset as tombstones for every node, and the Alpine CSP build
/// under the default policy: no `unsafe-*`, every `x-data` a registered component, no
/// inline handler anywhere.
#[tokio::test]
async fn test_animals_end_to_end() {
    let root = tempfile::tempdir().unwrap();
    configure(&root, LUA_CONFIG);
    let handler = handler_for(&root);
    let dev = node_id(&handler);
    let log = log_path(&handler, "animals");
    let seed = animals_seed();
    assert_eq!(seed.len(), 7);

    seed_animals(&handler).await;
    let lines = log_lines(&log);
    assert_eq!(lines.len(), 7);
    for (index, (line, wanted)) in lines.iter().zip(&seed).enumerate() {
        assert_eq!(line["seq"], index as u64 + 1, "{line}");
        assert_eq!(line["dev"], dev, "the seed is appended as this node's");
        assert_eq!(line["app"], "animals");
        assert_eq!(line["op"], "put");
        assert_eq!(line["tbl"], "node");
        assert_eq!(line["id"], wanted["id"]);
        assert_eq!(line["d"], wanted["d"]);
        assert!(is_ulid(line["id"].as_str().unwrap()), "{line}");
    }
    let by_text = |text: &str| -> String {
        seed.iter().find(|e| e["d"]["text"] == text).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let page = body_of(handler.handle(get("/settings/apps")).await).await;
    assert!(!page.contains("/settings/apps/animals/seed"), "{page}");
    assert!(page.contains("<code>node</code>: 7 rows"), "{page}");

    // Play starts at the root: no cursor row, the question nobody points at.
    let play = handler.handle(get("/a/animals/")).await;
    assert_eq!(play.status(), StatusCode::OK);
    let csp = header(&play, &CONTENT_SECURITY_POLICY).to_owned();
    assert!(!csp.contains("unsafe-"), "{csp}");
    assert!(csp.contains("script-src http://"), "{csp}");
    assert!(
        csp.contains("/a/animals/ "),
        "scoped to the app's path: {csp}"
    );
    let text = body_of(play).await;
    assert!(
        text.contains("<h1>Does it live in the water?</h1>"),
        "{text}"
    );
    assert!(text.contains("4 animals, 3 questions"), "{text}");
    assert!(
        text.contains(
            "<noscript><link rel=\"stylesheet\" href=\"/a/animals/static/nojs.css\"></noscript>"
        ),
        "{text}"
    );

    // An answer over htmx: the fragment alone, its forms carrying the token, the cursor
    // written, and the same question on a fresh GET.
    let answered = handler
        .handle(with_htmx(posted(
            &handler,
            "/a/animals/answer",
            "choice=yes",
        )))
        .await;
    assert_eq!(answered.status(), StatusCode::OK);
    let fragment = body_of(answered).await;
    assert!(!fragment.contains("<!doctype"), "{fragment}");
    assert!(!fragment.contains("pv-header"), "{fragment}");
    assert!(
        fragment.contains("<h1>Does it have fins?</h1>"),
        "{fragment}"
    );
    assert!(fragment.contains("name=\"_csrf\""), "{fragment}");
    for form in a11y::parse(&fragment).find_all("form") {
        assert_eq!(form.attr("method"), Some("post"), "{fragment}");
        assert_eq!(form.attr("action"), form.attr("hx-post"), "{fragment}");
        assert!(
            form.descendants()
                .iter()
                .any(|e| e.name == "input" && e.attr("name") == Some("_csrf")),
            "{fragment}"
        );
    }
    let lines = log_lines(&log);
    assert_eq!(lines.len(), 8);
    assert_eq!(lines[7]["tbl"], "cursor");
    assert_eq!(lines[7]["id"], "cursor");
    assert_eq!(lines[7]["d"]["node_id"], by_text("Does it have fins?"));
    assert!(lines[7]["d"]["started"].is_string(), "{}", lines[7]);
    let reloaded = body_of(handler.handle(get("/a/animals/")).await).await;
    assert!(
        reloaded.contains("<h1>Does it have fins?</h1>"),
        "{reloaded}"
    );

    // Down to a leaf.
    let leaf = body_of(
        handler
            .handle(with_htmx(posted(
                &handler,
                "/a/animals/answer",
                "choice=yes",
            )))
            .await,
    )
    .await;
    assert!(leaf.contains("<h1>Is it a shark?</h1>"), "{leaf}");
    assert_eq!(log_lines(&log).len(), 9);

    // Teach: the leaf becomes the question, keeping its id; three puts and the cursor's
    // tombstone in one batch — one ts, contiguous seqs.
    let teach = body_of(handler.handle(get("/a/animals/teach")).await).await;
    assert!(teach.contains("tells it apart from shark"), "{teach}");
    assert!(teach.contains("<label for=\"answer-yes\">"), "{teach}");
    assert!(teach.contains("id=\"answer-yes\""), "{teach}");
    let taught = handler
        .handle(posted(
            &handler,
            "/a/animals/teach",
            "animal=dolphin&question=Is+it+a+mammal%3F&answer=yes",
        ))
        .await;
    assert_eq!(taught.status(), StatusCode::SEE_OTHER);
    assert_eq!(header(&taught, &LOCATION), "/a/animals/");
    let lines = log_lines(&log);
    assert_eq!(lines.len(), 13);
    let batch = &lines[9..13];
    let ts: BTreeSet<&str> = batch.iter().map(|l| l["ts"].as_str().unwrap()).collect();
    assert_eq!(ts.len(), 1, "one batch, one ts: {batch:?}");
    let seqs: Vec<u64> = batch.iter().map(|l| l["seq"].as_u64().unwrap()).collect();
    assert_eq!(seqs, [10, 11, 12, 13]);
    assert_eq!(batch[0]["d"], json!({ "kind": "a", "text": "dolphin" }));
    assert_eq!(batch[1]["d"], json!({ "kind": "a", "text": "shark" }));
    assert_eq!(
        batch[2]["id"],
        by_text("shark"),
        "the leaf became the question"
    );
    assert_eq!(batch[2]["d"]["kind"], "q");
    assert_eq!(batch[2]["d"]["text"], "Is it a mammal?");
    assert_eq!(batch[2]["d"]["yes_id"], batch[0]["id"]);
    assert_eq!(batch[2]["d"]["no_id"], batch[1]["id"]);
    assert_eq!(batch[3]["op"], "del");
    assert_eq!(batch[3]["tbl"], "cursor");
    assert!(batch[3].get("d").is_none(), "{}", batch[3]);
    let dolphin_id = batch[0]["id"].as_str().unwrap().to_owned();
    let shark_id = batch[1]["id"].as_str().unwrap().to_owned();
    assert!(is_ulid(&dolphin_id) && is_ulid(&shark_id));

    // The knowledge page: the recursive walk, `->` between the questions.
    let knowledge = body_of(handler.handle(get("/a/animals/knowledge")).await).await;
    assert!(
        knowledge.contains("<th scope=\"col\">Animal</th>"),
        "{knowledge}"
    );
    for animal in ["shark", "otter", "parrot", "wombat", "dolphin"] {
        assert!(
            knowledge.contains(&format!("<td>{animal}</td>")),
            "{animal}: {knowledge}"
        );
    }
    assert!(
        knowledge.contains(
            "Does it live in the water?: yes -&gt; Does it have fins?: yes -&gt; Is it a mammal?: yes"
        ),
        "{knowledge}"
    );
    assert!(
        knowledge.contains(&format!("aria-controls=\"path-{dolphin_id}\"")),
        "{knowledge}"
    );
    assert!(
        knowledge.contains(&format!("id=\"path-{dolphin_id}\"")),
        "{knowledge}"
    );

    // The Alpine CSP build: every x-data names a registered component, and no page
    // carries an inline handler the policy would drop.
    let script = fs::read_to_string(
        repo_apps_dir()
            .join("animals")
            .join("static")
            .join("animals.js"),
    )
    .unwrap();
    let registered = alpine_components(&script);
    assert_eq!(
        registered,
        ["confirmable", "disclosure"]
            .into_iter()
            .map(str::to_owned)
            .collect()
    );
    for (name, html) in [
        ("play", &reloaded),
        ("teach", &teach),
        ("knowledge", &knowledge),
    ] {
        let used = x_data_values(html);
        assert!(
            used.is_subset(&registered),
            "{name}: x-data {used:?} not all registered {registered:?}"
        );
        let handlers: Vec<String> = a11y::check(html, Unit::Document)
            .into_iter()
            .filter(|f| f.starts_with("inline event handler"))
            .collect();
        assert!(handlers.is_empty(), "{name}: {handlers:?}");
    }
    assert!(!x_data_values(&knowledge).is_empty(), "{knowledge}");
    // Alpine's CDN builds call Alpine.start() in a microtask as soon as their script
    // runs, and start() dispatches `alpine:init` right then. A component registered from
    // an `alpine:init` listener in a script loaded *after* Alpine is registered too late
    // and every x-data is an "Undefined variable" (seen in Edge in M10). So animals.js
    // has to come before alpine-csp.min.js, and both are `defer`, which keeps that order.
    for (name, html) in [
        ("play", &reloaded),
        ("teach", &teach),
        ("knowledge", &knowledge),
    ] {
        let components = html
            .find("/static/animals.js")
            .unwrap_or_else(|| panic!("{name}: animals.js not loaded"));
        let alpine = html
            .find("/static/alpine-csp.min.js")
            .unwrap_or_else(|| panic!("{name}: alpine not loaded"));
        assert!(
            components < alpine,
            "{name}: animals.js must be loaded before Alpine, or alpine:init fires first"
        );
        let tree = a11y::parse(html);
        for script in tree.find_all("script") {
            assert_eq!(
                script.attr("defer"),
                Some(""),
                "{name}: every script is defer, so document order is execution order: {}",
                script.describe()
            );
        }
    }
    assert!(
        script.contains("alpine:init"),
        "animals.js registers its components on alpine:init"
    );

    // The app's own assets, beneath its mount.
    let alpine = handler
        .handle(get("/a/animals/static/alpine-csp.min.js"))
        .await;
    assert_eq!(alpine.status(), StatusCode::OK);
    assert!(header(&alpine, &CONTENT_TYPE).contains("javascript"));
    let nojs = handler.handle(get("/a/animals/static/nojs.css")).await;
    assert_eq!(nojs.status(), StatusCode::OK);
    assert!(header(&nojs, &CONTENT_TYPE).starts_with("text/css"));
    let nojs = body_of(nojs).await;
    assert!(nojs.contains("[x-cloak]"), "{nojs}");
    assert!(nojs.contains("revert"), "{nojs}");
    assert!(nojs.contains(".pv-js-only"), "{nojs}");

    // Reset: a tombstone for every node and the cursor, in one batch; the log keeps
    // every round played.
    let reset = handler
        .handle(posted(&handler, "/a/animals/reset", ""))
        .await;
    assert_eq!(reset.status(), StatusCode::SEE_OTHER);
    let lines = log_lines(&log);
    assert_eq!(lines.len(), 23);
    let tombstones = &lines[13..];
    assert!(
        tombstones.iter().all(|l| l["op"] == "del"),
        "{tombstones:?}"
    );
    let ts: BTreeSet<&str> = tombstones
        .iter()
        .map(|l| l["ts"].as_str().unwrap())
        .collect();
    assert_eq!(ts.len(), 1, "one batch: {tombstones:?}");
    let deleted: BTreeSet<String> = tombstones
        .iter()
        .filter(|l| l["tbl"] == "node")
        .map(|l| l["id"].as_str().unwrap().to_owned())
        .collect();
    let mut every_node: BTreeSet<String> = seed
        .iter()
        .map(|e| e["id"].as_str().unwrap().to_owned())
        .collect();
    every_node.insert(dolphin_id);
    every_node.insert(shark_id);
    assert_eq!(deleted, every_node);
    assert_eq!(tombstones.last().unwrap()["tbl"], "cursor");
    let empty = body_of(handler.handle(get("/a/animals/")).await).await;
    assert!(empty.contains("I don't know any animals yet."), "{empty}");
    assert!(empty.contains("0 animals, 0 questions"), "{empty}");
}

/// `docs/plans/phase-1.md` M10 — every write in animals as a plain form post with no
/// `HX-Request`: each answers 303 to the board and the next GET shows the state; the
/// error branch renders the page with `role="alert"`; every board form carries `method`
/// and `action` beside `hx-post`; the reset form is a real form with `csrf()`; the
/// `<noscript>` sheet is linked and reveals what Alpine hides.
#[tokio::test]
async fn test_animals_works_with_javascript_disabled() {
    let root = tempfile::tempdir().unwrap();
    configure(&root, LUA_CONFIG);
    let handler = handler_for(&root);
    let log = log_path(&handler, "animals");

    let plain = |path: &str, form: &str| posted(&handler, path, form);
    let redirected = |response: &Response| {
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(header(response, &LOCATION), "/a/animals/");
    };
    let board = || async { body_of(handler.handle(get("/a/animals/")).await).await };

    let planted = handler
        .handle(plain("/a/animals/seed", "animal=wombat"))
        .await;
    redirected(&planted);
    assert!(board().await.contains("<h1>Is it a wombat?</h1>"));
    let started = handler.handle(plain("/a/animals/start", "")).await;
    redirected(&started);
    assert!(board().await.contains("<h1>Is it a wombat?</h1>"));
    let taught = handler
        .handle(plain(
            "/a/animals/teach",
            "animal=penguin&question=Does+it+fly%3F&answer=no",
        ))
        .await;
    redirected(&taught);
    assert!(board().await.contains("<h1>Does it fly?</h1>"));
    let answered = handler
        .handle(plain("/a/animals/answer", "choice=yes"))
        .await;
    redirected(&answered);
    assert!(board().await.contains("<h1>Is it a wombat?</h1>"));
    let answered = handler
        .handle(plain("/a/animals/answer", "choice=no"))
        .await;
    redirected(&answered);
    assert!(
        board().await.contains("<h1>Is it a wombat?</h1>"),
        "a leaf stays a leaf"
    );

    // The board's forms: method and action beside hx-post, the token in each.
    let page = board().await;
    let forms = a11y::parse(&page).find_all("form").len();
    assert!(forms >= 1, "{page}");
    for form in a11y::parse(&page).find_all("form") {
        assert_eq!(form.attr("method"), Some("post"), "{page}");
        let action = form.attr("action").unwrap();
        assert!(action.starts_with("/a/animals/"), "{page}");
        assert_eq!(form.attr("hx-post"), Some(action), "{page}");
        assert!(
            form.descendants()
                .iter()
                .any(|e| e.name == "input" && e.attr("name") == Some("_csrf")),
            "{page}"
        );
    }
    assert!(
        page.contains(
            "<noscript><link rel=\"stylesheet\" href=\"/a/animals/static/nojs.css\"></noscript>"
        ),
        "{page}"
    );

    // What Alpine hides is reachable: the reset form is a real form with the token, the
    // toggles are marked JS-only, the hidden parts carry x-cloak for nojs.css to revert.
    let knowledge = body_of(handler.handle(get("/a/animals/knowledge")).await).await;
    assert!(
        knowledge.contains("<form method=\"post\" action=\"/a/animals/reset\">"),
        "{knowledge}"
    );
    let tree = a11y::parse(&knowledge);
    let reset_form = tree
        .find_all("form")
        .into_iter()
        .find(|f| f.attr("action") == Some("/a/animals/reset"))
        .unwrap();
    assert!(
        reset_form
            .descendants()
            .iter()
            .any(|e| e.name == "input" && e.attr("name") == Some("_csrf")),
        "{knowledge}"
    );
    let animals = tree.find_all("tbody")[0].find_all("tr").len();
    assert_eq!(animals, 2, "wombat and penguin: {knowledge}");
    let toggles: Vec<&a11y::Element> = tree
        .descendants()
        .into_iter()
        .filter(|e| e.name == "button" && e.attr("x-on:click").is_some())
        .collect();
    assert_eq!(
        toggles.len(),
        animals + 2,
        "a show-path per animal, forget, keep: {knowledge}"
    );
    for toggle in toggles {
        assert!(toggle.has_class("pv-js-only"), "{}", toggle.describe());
    }
    let cloaked = tree
        .descendants()
        .into_iter()
        .filter(|e| e.attr("x-cloak").is_some())
        .count();
    assert_eq!(
        cloaked,
        animals + 1,
        "a path per animal and the confirmation: {knowledge}"
    );
    let teach = body_of(handler.handle(get("/a/animals/teach")).await).await;
    let tree = a11y::parse(&teach);
    let toggle = tree
        .descendants()
        .into_iter()
        .find(|e| e.name == "button" && e.attr("x-on:click") == Some("toggle"))
        .unwrap();
    assert!(toggle.has_class("pv-js-only"), "{teach}");
    assert!(teach.contains("nojs.css"), "{teach}");
    let nojs = body_of(handler.handle(get("/a/animals/static/nojs.css")).await).await;
    let rules: BTreeMap<String, BTreeMap<String, String>> =
        a11y::rules(&nojs).into_iter().collect();
    assert!(
        rules["[x-cloak]"]["display"].starts_with("revert"),
        "{nojs}"
    );
    assert!(
        rules[".pv-js-only"]["display"].starts_with("none"),
        "{nojs}"
    );

    // Forget, as a plain post; then the error branch, a page with the alert in it.
    let reset = handler.handle(plain("/a/animals/reset", "")).await;
    redirected(&reset);
    assert!(board().await.contains("I don't know any animals yet."));
    let before = log_lines(&log).len();
    let refused = handler.handle(plain("/a/animals/seed", "animal=+")).await;
    assert_eq!(refused.status(), StatusCode::OK);
    let text = body_of(refused).await;
    assert!(text.starts_with("<!doctype html>"), "{text}");
    assert!(text.contains("role=\"alert\""), "{text}");
    assert!(text.contains("Name any animal."), "{text}");
    assert!(
        text.contains("<h1>I don't know any animals yet.</h1>"),
        "{text}"
    );
    assert_eq!(log_lines(&log).len(), before, "nothing written");
}

// ---------------------------------------------------------------------------------------
// sketch
// ---------------------------------------------------------------------------------------

/// `apps/sketch` — exactly the calls `app.js` makes, against the log: the page and its
/// two files plus `pv.js`; the boot read of `/api/events?tbl=stroke`; the stream; a
/// `pv.put` whose frame is the log line; a clear as a `del` batch; the boot read again,
/// which applies the put and then the del; a line appended by hand, which the next read
/// returns and the stream reports as a resync; `/api/node`. And the page itself: no
/// inline script or style, a zoomable viewport, a labelled canvas.
#[tokio::test]
async fn test_sketch_end_to_end() {
    let root = tempfile::tempdir().unwrap();
    let mut handler = handler_for(&root);
    handler.api_mut().set_ping(Duration::from_millis(200));
    let dev = node_id(&handler);
    let log = log_path(&handler, "sketch");
    let web = repo_apps_dir().join("sketch").join("web");

    let index = handler.handle(get("/a/sketch/")).await;
    assert_eq!(index.status(), StatusCode::OK);
    assert!(header(&index, &CONTENT_TYPE).starts_with("text/html"));
    let index = body_of(index).await;
    assert_eq!(index, fs::read_to_string(web.join("index.html")).unwrap());
    assert!(index.contains("<html lang=\"en\">"), "{index}");
    assert!(
        index.contains("<canvas id=\"pad\" aria-label=\""),
        "{index}"
    );
    assert!(!index.contains("user-scalable=no"), "{index}");
    assert!(index.contains("<meta name=\"viewport\""), "{index}");
    assert!(
        index.contains("<script type=\"module\" src=\"app.js\">"),
        "{index}"
    );
    assert_clean("sketch index.html", &index, Unit::Document);
    for (file, kind) in [("app.js", "javascript"), ("style.css", "text/css")] {
        let response = handler.handle(get(&format!("/a/sketch/{file}"))).await;
        assert_eq!(response.status(), StatusCode::OK, "{file}");
        assert!(header(&response, &CONTENT_TYPE).contains(kind), "{file}");
        assert_eq!(
            body_of(response).await,
            fs::read_to_string(web.join(file)).unwrap(),
            "{file}"
        );
    }
    let pv = handler.handle(get("/static/pv.js")).await;
    assert_eq!(pv.status(), StatusCode::OK);
    let pv = body_of(pv).await;
    assert!(
        pv.len() < 12 * 1024,
        "pv.js is {} bytes; spec/data-api.md §5 says under 12 KB",
        pv.len()
    );
    let app_js = fs::read_to_string(web.join("app.js")).unwrap();
    for call in [
        "pv.events({ tbl: 'stroke' })",
        "pv.put(",
        "pv.append(",
        "pv.subscribe(",
        "pv.on('resync'",
        "pv.ulid()",
    ] {
        assert!(app_js.contains(call), "app.js no longer calls {call}");
    }
    assert!(
        app_js.contains("clientWidth"),
        "the backing store follows the CSS size"
    );
    assert!(
        app_js.contains("aria-pressed"),
        "the current colour is announced"
    );

    // Boot on an empty log.
    let boot = body_of(handler.handle(get("/a/sketch/api/events?tbl=stroke")).await).await;
    assert_eq!(boot, "");

    // The stream, then a stroke: `pv.put('stroke', id, drawing)`.
    let mut stream = handler
        .handle(get("/a/sketch/api/stream"))
        .await
        .into_body();
    let id = ulid();
    let drawing = json!({ "points": [[10, 10], [40, 90]], "color": "#00274C", "width": 3 });
    let (status, body) = json_of(
        handler
            .handle(post_json(
                "/a/sketch/api/events",
                &json!({ "events": [{ "op": "put", "tbl": "stroke", "id": id, "d": drawing }] }),
            ))
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["ids"][0], id);
    assert_eq!(body["lam"], 1);
    let (event, data) = next_frame(&mut stream, 5).await.unwrap();
    assert_eq!(event, "append");
    let raw = fs::read_to_string(&log).unwrap();
    assert_eq!(
        data,
        raw.lines().next().unwrap(),
        "the frame is the log line"
    );
    let lines = log_lines(&log);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["seq"], 1);
    assert_eq!(lines[0]["lam"], 1);
    assert_eq!(lines[0]["dev"], dev);
    assert_eq!(lines[0]["app"], "sketch");
    assert_eq!(lines[0]["tbl"], "stroke");
    assert_eq!(
        lines[0]["d"], drawing,
        "stored as given: no schema, no validation"
    );

    // Clear: one batch of dels.
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
    let del: Value = serde_json::from_str(&data).unwrap();
    assert_eq!(del["op"], "del");
    assert_eq!(del["id"], id);
    assert!(del.get("d").is_none(), "{del}");

    // Boot again: the log in order, a put then a del, so nothing is drawn.
    let boot = body_of(handler.handle(get("/a/sketch/api/events?tbl=stroke")).await).await;
    let mut strokes: BTreeMap<String, Value> = BTreeMap::new();
    for line in boot.lines() {
        let ev: Value = serde_json::from_str(line).unwrap();
        let ev_id = ev["id"].as_str().unwrap().to_owned();
        if ev["op"] == "del" {
            strokes.remove(&ev_id);
        } else {
            strokes.insert(ev_id, ev["d"].clone());
        }
    }
    assert_eq!(boot.lines().count(), 2, "{boot}");
    assert!(strokes.is_empty());

    // A line appended by hand: the next boot read returns it, and the stream says resync.
    let by_hand = ulid();
    let ts = privatium_core::log::format_ts(jiff::Timestamp::now());
    let line = format!(
        r##"{{"seq":3,"lam":3,"ts":"{ts}","dev":"{dev}","app":"sketch","op":"put","tbl":"stroke","id":"{by_hand}","d":{{"points":[[0,0],[5,5]],"color":"#FFCB05","width":2}}}}"##
    );
    hand_append(&log, &line, "\n");
    let boot = body_of(handler.handle(get("/a/sketch/api/events?tbl=stroke")).await).await;
    assert_eq!(boot.lines().count(), 3, "{boot}");
    assert_eq!(boot.lines().last().unwrap(), line);
    let mut seen = Vec::new();
    for _ in 0..6 {
        match next_frame(&mut stream, 5).await {
            Some(frame) => {
                let stop = frame.0 == "resync";
                seen.push(frame);
                if stop {
                    break;
                }
            }
            None => break,
        }
    }
    assert!(
        seen.iter()
            .any(|(event, data)| event == "resync" && data.contains("\"lam\":3")),
        "{seen:?}"
    );

    // `pv.node()`.
    let (status, node) = json_of(handler.handle(get("/a/sketch/api/node")).await).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(node["id"], dev, "{node}");
}

// ---------------------------------------------------------------------------------------
// The accessibility baseline (spec/cli.md §5.4)
// ---------------------------------------------------------------------------------------

/// `spec/cli.md §5.4` — the framework's own pages meet `PV401`–`PV407`: the launcher with
/// a mounted, an unavailable and a missing app; the four settings pages with a permission
/// widening, an unknown icon, a broken folder, a seed offer and an alert so the alerts
/// table renders; the 404; the error page; a Lua error page with source context; the 503
/// for a build without a Lua host. `lang`, one `<main>`, labelled `<nav>`s, the skip
/// target, headings in order with one h1, labels, named controls, `<th scope>`, status
/// regions carrying text, no `on*=`, no `style=`, no inline script.
#[tokio::test]
async fn test_spec_cli_5_pv4xx_shell_pages() {
    let root = tempfile::tempdir().unwrap();
    configure(&root, LUA_CONFIG);
    let apps = privatium_core::Paths::rooted(root.path()).apps_dir();
    write_app(
        &apps,
        "wide",
        Some(&format!(
            "{}icon = \"no-such-icon\"\n[permissions]\nsql = true\n",
            lua_manifest("wide")
        )),
        &[("app.lua", "")],
    );
    write_app(&apps, "broken", Some("not toml at all ["), &[]);
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
                "{\"op\":\"put\",\"tbl\":\"profile\",\"id\":\"a\",\"d\":{\"display_name\":\"Ada\"}}\n",
            ),
        ],
    );
    write_app(
        &apps,
        "boom",
        Some(&lua_manifest("boom")),
        &[(
            "app.lua",
            "local pv = require 'privatium'\npv.get('/', function()\n  error('boom')\nend)\n",
        )],
    );
    let gone = write_web_app(&apps, "gone", &[]);
    {
        let (node, _) = open(&root);
        assert!(node.app("gone").is_some());
    }
    fs::remove_dir_all(&gone).unwrap();
    let handler = handler_for(&root);
    assert!(handler.report().missing.contains(&"gone".to_owned()));
    {
        let mut node = handler.node().lock().unwrap();
        let at = privatium_core::log::now();
        node.sys_log_mut()
            .put(
                privatium_core::sys::AUDIT,
                "01J6ZK2Q000000000000000ART",
                &json!({
                    "at": at,
                    "kind": "restore.tier3",
                    "actor": "system",
                    "subject": "hello",
                    "detail": "{\"reason\":\"a test alert\"}",
                    "severity": "alert",
                }),
            )
            .unwrap();
        node.refresh().unwrap();
    }

    let mut pages: Vec<(String, String)> = Vec::new();
    for (path, status) in [
        ("/", StatusCode::OK),
        ("/settings", StatusCode::OK),
        ("/settings/apps", StatusCode::OK),
        ("/settings/data", StatusCode::OK),
        ("/settings/devices", StatusCode::OK),
        ("/nope", StatusCode::NOT_FOUND),
        ("/a/boom/", StatusCode::INTERNAL_SERVER_ERROR),
    ] {
        let response = handler.handle(get(path)).await;
        assert_eq!(response.status(), status, "{path}");
        pages.push((path.to_owned(), body_of(response).await));
    }
    pages.push((
        "shell::error".into(),
        shell::error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "the detail <b>escaped</b>",
            false,
        ),
    ));
    pages.push(("shell::no_handler".into(), shell::no_handler("hello", true)));
    pages.push(("shell::not_found solo".into(), shell::not_found("/x", true)));
    for (path, html) in &pages {
        assert_clean(path, html, Unit::Document);
    }
    let page = |path: &str| -> &str { &pages.iter().find(|(p, _)| p == path).unwrap().1 };
    // What the fixtures were for: every branch of every page really rendered.
    let launcher = page("/");
    assert!(launcher.contains("href=\"/a/hello/\""), "{launcher}");
    assert!(launcher.contains("pv-unavailable"), "{launcher}");
    assert!(
        launcher.contains("gone — unavailable: folder missing"),
        "{launcher}"
    );
    assert!(
        launcher.contains("includeIndicatorStyles\":false"),
        "{launcher}"
    );
    let node_page = page("/settings");
    assert!(
        node_page.contains("<nav aria-label=\"Settings\">"),
        "{node_page}"
    );
    assert!(
        node_page.contains("<th scope=\"col\">When</th>"),
        "{node_page}"
    );
    assert!(node_page.contains("restore.tier3"), "{node_page}");
    let apps_page = page("/settings/apps");
    for expected in [
        "Load sample data",
        "Load warnings",
        "ad-hoc read-only SQL",
        "not in the vendored Bootstrap Icons set",
        "Not loaded at startup",
        "<code>broken</code>",
    ] {
        assert!(apps_page.contains(expected), "{expected}\n{apps_page}");
    }
    let devices = page("/settings/devices");
    assert!(
        devices.contains("<th scope=\"col\">Device</th>"),
        "{devices}"
    );
    let boom = page("/a/boom/");
    assert!(boom.contains("<mark aria-current=\"true\">"), "{boom}");
    assert!(boom.contains("pv-source"), "{boom}");
    assert!(boom.contains("boom"), "{boom}");
    assert!(page("/nope").contains("<code>/nope</code>"));
}

/// `spec/cli.md §5.4` — the Tier 1 page frame and the reference views meet the same
/// rules: hello's greeting, form and error re-render; animals' board, teach and knowledge
/// pages with the seed loaded; the `_board` fragment on its own (element checks only —
/// the page it lands in supplies the document); sketch's `index.html` as served.
#[tokio::test]
async fn test_spec_cli_5_pv4xx_app_frame_and_reference_views() {
    let root = tempfile::tempdir().unwrap();
    configure(&root, LUA_CONFIG);
    let handler = handler_for(&root);
    seed_animals(&handler).await;

    let mut documents: Vec<(&str, String)> = Vec::new();
    for path in [
        "/a/hello/",
        "/a/hello/edit",
        "/a/animals/",
        "/a/animals/teach",
        "/a/animals/knowledge",
    ] {
        let response = handler.handle(get(path)).await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        documents.push((path, body_of(response).await));
    }
    let error = handler
        .handle(posted(&handler, "/a/hello/name", "display_name=+"))
        .await;
    assert_eq!(error.status(), StatusCode::OK);
    documents.push(("/a/hello/name (error re-render)", body_of(error).await));
    let named = handler
        .handle(posted(&handler, "/a/hello/name", "display_name=Ada"))
        .await;
    assert_eq!(named.status(), StatusCode::SEE_OTHER);
    documents.push((
        "/a/hello/ (greeting)",
        body_of(handler.handle(get("/a/hello/")).await).await,
    ));
    documents.push((
        "sketch web/index.html",
        fs::read_to_string(
            repo_apps_dir()
                .join("sketch")
                .join("web")
                .join("index.html"),
        )
        .unwrap(),
    ));
    for (name, html) in &documents {
        assert_clean(name, html, Unit::Document);
    }
    assert!(
        documents[4].1.contains("aria-controls=\"path-"),
        "{}",
        documents[4].1
    );
    assert!(
        documents[5].1.contains("role=\"alert\""),
        "{}",
        documents[5].1
    );

    let fragment = body_of(
        handler
            .handle(with_htmx(posted(&handler, "/a/animals/start", "")))
            .await,
    )
    .await;
    assert!(
        fragment.contains("<h1>Does it live in the water?</h1>"),
        "{fragment}"
    );
    assert_clean("_board fragment", &fragment, Unit::Fragment);
}

/// `PV406` — the declared colour tokens of the shell's stylesheet meet 4.5:1 for text and
/// 3:1 for focus and control boundaries, in both colour schemes; `:focus-visible` draws
/// an outline and nothing removes one without a replacement; sketch's own colours
/// likewise.
#[test]
fn test_spec_cli_5_pv406_declared_tokens_meet_contrast() {
    let css = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join("shell")
            .join("shell.css"),
    )
    .unwrap();
    let schemes = a11y::root_tokens(&css);
    assert_eq!(schemes.len(), 2, "a light and a dark :root");
    let text_pairs = [
        ("--pv-fg", "--pv-bg"),
        ("--pv-fg", "--pv-panel"),
        ("--pv-fg", "--pv-line"),
        ("--pv-muted", "--pv-bg"),
        ("--pv-muted", "--pv-panel"),
        ("--pv-accent-fg", "--pv-accent"),
        ("--pv-warn-fg", "--pv-warn-bg"),
        ("--pv-alert-fg", "--pv-alert-bg"),
        ("--pv-ok-fg", "--pv-ok-bg"),
    ];
    let ui_pairs = [
        ("--pv-focus", "--pv-bg"),
        ("--pv-focus", "--pv-panel"),
        ("--pv-muted", "--pv-bg"),
        ("--pv-muted", "--pv-panel"),
        ("--pv-accent", "--pv-bg"),
        ("--pv-accent", "--pv-panel"),
        ("--pv-alert-fg", "--pv-bg"),
    ];
    for (name, tokens) in [("light", &schemes[0]), ("dark", &schemes[1])] {
        for (fg, bg) in text_pairs {
            let ratio = a11y::contrast(&tokens[fg], &tokens[bg]);
            assert!(
                ratio >= 4.5,
                "{name}: {fg} on {bg} is {ratio:.2}:1, want 4.5"
            );
        }
        for (fg, bg) in ui_pairs {
            let ratio = a11y::contrast(&tokens[fg], &tokens[bg]);
            assert!(ratio >= 3.0, "{name}: {fg} on {bg} is {ratio:.2}:1, want 3");
        }
    }
    let rules = a11y::rules(&css);
    let focus = rules
        .iter()
        .find(|(selector, _)| selector.contains(":focus-visible"))
        .expect(":focus-visible rule");
    let outline = focus.1.get("outline").expect("an outline");
    assert!(
        !outline.starts_with("none") && !outline.starts_with('0'),
        "{outline}"
    );
    assert!(outline.contains("var(--pv-focus)"), "{outline}");
    for (selector, declarations) in &rules {
        if let Some(outline) = declarations.get("outline")
            && (outline.starts_with("none") || outline.trim() == "0")
        {
            assert!(
                declarations.contains_key("box-shadow") || declarations.contains_key("border"),
                "{selector} removes the outline with no replacement"
            );
        }
    }
    let inputs = rules
        .iter()
        .find(|(selector, _)| selector.starts_with("main form input"))
        .expect("the text input rule");
    assert!(
        inputs.1["border"].contains("var(--pv-muted)"),
        "{}",
        inputs.1["border"]
    );
    assert!(
        !rules.iter().any(|(_, d)| d.contains_key("opacity")),
        "no rule dims text below its token's contrast"
    );
    assert!(css.contains("prefers-reduced-motion"), "the motion guard");

    // sketch: its own sheet, its own colours.
    let sketch =
        fs::read_to_string(repo_apps_dir().join("sketch").join("web").join("style.css")).unwrap();
    let tokens = &a11y::root_tokens(&sketch)[0];
    let navy = &tokens["--navy"];
    let muted = &tokens["--muted"];
    assert!(a11y::contrast(navy, "#ffffff") >= 3.0, "focus on the page");
    assert!(
        a11y::contrast(navy, "#fafafa") >= 3.0,
        "focus on the toolbar"
    );
    assert!(
        a11y::contrast(navy, "#ffffff") >= 4.5,
        "the clear button's text"
    );
    assert!(a11y::contrast(muted, "#fafafa") >= 4.5, "the status text");
    assert!(a11y::contrast("#1d2129", "#ffffff") >= 4.5, "body text");
    let rules = a11y::rules(&sketch);
    let focus = rules
        .iter()
        .find(|(selector, _)| selector.contains(":focus-visible"))
        .expect(":focus-visible rule");
    assert!(focus.1["outline"].contains("var(--navy)"), "{:?}", focus.1);
    assert!(
        rules
            .iter()
            .any(|(selector, _)| selector.contains("[aria-pressed=\"true\"]")),
        "the current swatch is drawn from aria-pressed"
    );
    assert!(!sketch.contains("user-scalable"), "{sketch}");
}
