// Project:  Privatium™  |  File: crates/privatium-core/tests/lua.rs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-03  |  Modified: 2026-09-03
// Summary:  The Lua host against spec/lua-api.md and docs/plans/phase-1.md M7, every test
//           through core::handle with no listener: the sandbox of §5 and its four limits,
//           adversarially (R2); the stable route index of §2.4; the pv module of §3 —
//           routing, typed reads, appends and batches, pv.dec; the sandbox globals of §4.0;
//           solo-mode shadowing; and the two reference apps loading and answering.

// AGENTS.md, Style: unwrap() is permitted in tests, and a test that hides a failure
// behind `?` is worse than one that panics with a line number.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::fs;
use std::path::Path;

use axum::body::{Body, to_bytes};
use axum::http::header::{ALLOW, CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, LOCATION};
use axum::http::{Method, StatusCode};
use common::{audit_rows, log_lines, lua_manifest, repo_apps_dir, write_app, write_lua_app};
use privatium_core::app::Warning;
use privatium_core::{AppRoot, Handler, LoadReport, Node, Request, Response, Stage};
use serde_json::Value;

/// The `[lua]` table every test writes, small enough to trip quickly and to build a pool
/// of two VMs in milliseconds. Individual tests override a line.
const LUA_CONFIG: &str =
    "[lua]\npool_size = 2\nmax_instructions = 5000000\nmax_memory_mb = 16\nmax_seconds = 20\n";

fn configure(root: &tempfile::TempDir, config: &str) {
    fs::write(root.path().join("config.toml"), config).unwrap();
}

/// A node with the owner's `apps/` loaded. `bundled` adds the repository's reference apps.
fn open(root: &tempfile::TempDir, bundled: bool) -> (Node, LoadReport) {
    let mut node = Node::open(root.path()).unwrap();
    let mut roots = vec![AppRoot::local(node.paths().apps_dir())];
    if bundled {
        roots.push(AppRoot::bundled(repo_apps_dir()));
    }
    let report = node.load_apps(&roots).unwrap();
    (node, report)
}

fn handler_for(root: &tempfile::TempDir) -> Handler {
    let (node, report) = open(root, false);
    Handler::new(node, report)
}

/// Write `app.lua` for `slug` under the root's `apps/`, plus `files`.
fn app(root: &tempfile::TempDir, slug: &str, app_lua: &str, files: &[(&str, &str)]) {
    let apps = Node::open(root.path()).unwrap().paths().apps_dir();
    let mut all = vec![("app.lua", app_lua)];
    all.extend_from_slice(files);
    write_lua_app(&apps, slug, &all);
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

async fn body_of(response: Response) -> String {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    String::from_utf8_lossy(&bytes).into_owned()
}

async fn json_of(response: Response) -> Value {
    let status = response.status();
    let text = body_of(response).await;
    assert_eq!(status, StatusCode::OK, "{text}");
    serde_json::from_str(&text).unwrap()
}

fn header<'a>(response: &'a Response, name: &axum::http::HeaderName) -> &'a str {
    response
        .headers()
        .get(name)
        .map(|v| v.to_str().unwrap())
        .unwrap_or_default()
}

fn log_path(handler: &Handler, slug: &str) -> std::path::PathBuf {
    let node = handler.node().lock().unwrap();
    node.paths().app_log(slug, node.id())
}

/// The `lua.limit_exceeded` rows, after refreshing `_sys` so the append is visible.
fn limit_audits(handler: &Handler) -> Vec<Value> {
    let mut node = handler.node().lock().unwrap();
    node.refresh().unwrap();
    audit_rows(&node, "lua.limit_exceeded")
}

/// A schema exercising the typing rule of `§3.2`.
const TYPED_DDL: &str = "CREATE TABLE thing (
    id        VARCHAR PRIMARY KEY,
    name      VARCHAR,
    amount    DECIMAL(18,2),
    big       BIGINT,
    ok        BOOLEAN,
    tags      VARCHAR[],
    filled_on DATE
);
CREATE VIEW v_thing AS SELECT id, big AS b, amount AS a FROM thing;";

/// An app whose `/ok` route always answers, for the limit tests.
const OK_ROUTE: &str = "pv.get('/ok', function() return pv.text('ok') end)\n";

// ---------------------------------------------------------------------------------------
// §5 — the sandbox
// ---------------------------------------------------------------------------------------

/// `spec/lua-api.md §5` — one assertion per banned name (the table's fourteen, plus
/// `os.setlocale`, which this build removes too), and the retained set present.
#[tokio::test]
async fn test_spec_lua_5_banned_globals_absent() {
    let root = tempfile::tempdir().unwrap();
    configure(&root, LUA_CONFIG);
    app(
        &root,
        "sand",
        r#"
local pv = require 'privatium'
pv.get('/probe', function()
  return pv.json({
    io = io ~= nil,
    os_execute = os.execute ~= nil, os_exit = os.exit ~= nil, os_getenv = os.getenv ~= nil,
    os_remove = os.remove ~= nil, os_rename = os.rename ~= nil, os_tmpname = os.tmpname ~= nil,
    os_setlocale = os.setlocale ~= nil,
    package_loadlib = package.loadlib ~= nil, package_cpath = package.cpath ~= nil,
    debug = debug ~= nil,
    load = load ~= nil, loadstring = loadstring ~= nil, dofile = dofile ~= nil,
    loadfile = loadfile ~= nil,
    -- retained
    os_time = type(os.time), os_date = type(os.date), os_clock = type(os.clock),
    string = type(string), table = type(table), math = type(math),
    coroutine = type(coroutine), utf8 = type(utf8), require = type(require),
    package_loaded = type(package.loaded), print = type(print),
    url = type(url), icon = type(icon), fmt = type(fmt), t = type(t),
    -- template-only helpers are not handler globals
    render = render ~= nil, layout = layout ~= nil, csrf = csrf ~= nil,
  })
end)
"#,
        &[],
    );
    let handler = handler_for(&root);
    let probe = json_of(handler.handle(get("/a/sand/probe")).await).await;

    assert_eq!(probe["io"], false);
    assert_eq!(probe["os_execute"], false);
    assert_eq!(probe["os_exit"], false);
    assert_eq!(probe["os_getenv"], false);
    assert_eq!(probe["os_remove"], false);
    assert_eq!(probe["os_rename"], false);
    assert_eq!(probe["os_tmpname"], false);
    assert_eq!(probe["package_loadlib"], false);
    assert_eq!(probe["package_cpath"], false);
    assert_eq!(probe["debug"], false);
    assert_eq!(probe["load"], false);
    assert_eq!(probe["loadstring"], false);
    assert_eq!(probe["dofile"], false);
    assert_eq!(probe["loadfile"], false);
    assert_eq!(probe["os_setlocale"], false);

    for retained in [
        "os_time", "os_date", "os_clock", "require", "print", "url", "icon", "t",
    ] {
        assert_eq!(probe[retained], "function", "{retained}");
    }
    for retained in [
        "string",
        "table",
        "math",
        "coroutine",
        "utf8",
        "package_loaded",
        "fmt",
    ] {
        assert_eq!(probe[retained], "table", "{retained}");
    }
    assert_eq!(probe["render"], false);
    assert_eq!(probe["layout"], false);
    assert_eq!(probe["csrf"], false);
}

/// `spec/lua-api.md §5` — `require` serves `lib/` and `'privatium'` and nothing else:
/// dotted names, and no `../`, no absolute path, no `..` inside a dotted name, and no
/// symlink out of `lib/` where the platform lets the test create one.
#[tokio::test]
async fn test_spec_lua_5_require_confined_to_lib() {
    let root = tempfile::tempdir().unwrap();
    configure(&root, LUA_CONFIG);
    app(
        &root,
        "req",
        r#"
local pv = require 'privatium'
local same = require 'privatium'
pv.get('/probe', function(req)
  local ok, value = pcall(require, req.query.name)
  local kind = ok and type(value) or 'error'
  local detail = ok and (type(value) == 'table' and (value.name or '') or tostring(value)) or tostring(value)
  return pv.json({ ok = ok, kind = kind, detail = detail, same = same == pv })
end)
"#,
        &[
            ("lib/tree.lua", "return { name = 'tree' }"),
            ("lib/shared/dates.lua", "return { name = 'dates' }"),
            ("lib/nothing.lua", "local x = 1"),
            ("secret.lua", "return { name = 'SECRET' }"),
            ("lib/../outside.lua", "return { name = 'OUTSIDE' }"),
        ],
    );
    // A symlink from inside lib/ to a file outside it. Creating one needs a privilege on
    // Windows that a normal account may lack; the case is skipped, not faked, then.
    let lib = Node::open(root.path())
        .unwrap()
        .paths()
        .apps_dir()
        .join("req")
        .join("lib");
    let symlinked = symlink(&lib.join("..").join("secret.lua"), &lib.join("evil.lua"));
    let handler = handler_for(&root);

    let probe = |name: &str| {
        let path = format!("/a/req/probe?name={}", name.replace('/', "%2F"));
        let handler = &handler;
        async move { json_of(handler.handle(get(&path)).await).await }
    };
    let tree = probe("tree").await;
    assert_eq!(tree["ok"], true, "{tree}");
    assert_eq!(tree["detail"], "tree");
    assert_eq!(tree["same"], true, "require 'privatium' is one table");
    let dates = probe("shared.dates").await;
    assert_eq!(dates["ok"], true, "{dates}");
    assert_eq!(dates["detail"], "dates");
    let nothing = probe("nothing").await;
    assert_eq!(
        nothing["kind"], "boolean",
        "a module returning nothing is `true`"
    );

    for (name, expected) in [
        ("../secret", "not a module name"),
        ("/secret", "not a module name"),
        ("lib..tree", "not a module name"),
        ("..tree", "not a module name"),
        ("secret", "not found in lib/"),
        ("outside", "not found in lib/"),
        ("privatium.internal", "not found in lib/"),
        ("", "not a module name"),
    ] {
        let refused = probe(name).await;
        assert_eq!(refused["ok"], false, "{name}: {refused}");
        assert!(
            refused["detail"].as_str().unwrap().contains(expected),
            "{name}: {refused}"
        );
        assert!(!refused["detail"].as_str().unwrap().contains("SECRET"));
    }
    if symlinked {
        let evil = probe("evil").await;
        assert_eq!(evil["ok"], false, "{evil}");
        assert!(
            evil["detail"]
                .as_str()
                .unwrap()
                .contains("resolves outside lib/"),
            "{evil}"
        );
    }
}

#[cfg(unix)]
fn symlink(target: &Path, link: &Path) -> bool {
    std::os::unix::fs::symlink(target, link).is_ok()
}

#[cfg(windows)]
fn symlink(target: &Path, link: &Path) -> bool {
    std::os::windows::fs::symlink_file(target, link).is_ok()
}

/// `spec/lua-api.md §5`, R2 — the instruction limit aborts a tight loop, a loop that
/// swallows the error with `pcall` however the count lines up, a loop inside a coroutine,
/// and a loop inside a C-called callback; each is a 500, a `lua.limit_exceeded` row, and
/// then a healthy next request on the same pool.
#[tokio::test]
async fn test_spec_lua_5_instruction_limit_aborts() {
    let root = tempfile::tempdir().unwrap();
    configure(
        &root,
        "[lua]\npool_size = 2\nmax_instructions = 300000\nmax_memory_mb = 32\nmax_seconds = 30\n",
    );
    app(
        &root,
        "spin",
        &format!(
            r#"
local pv = require 'privatium'
{OK_ROUTE}
pv.get('/loop', function() while true do end end)
pv.get('/pcall', function()
  -- Catches the limit error every time, and keeps going: the escalated hook must reach
  -- the outer loop's own instructions.
  while true do pcall(function() while true do end end) end
end)
pv.get('/aligned', function()
  -- The inner body runs a fixed number of instructions so the count boundary can land in
  -- the same place every iteration.
  local function inner() for i = 1, 250 do end end
  while true do pcall(inner) end
end)
pv.get('/coroutine', function()
  local co = coroutine.wrap(function() while true do coroutine.yield() end end)
  while true do co() end
end)
pv.get('/resume', function()
  local co = coroutine.create(function() while true do pcall(function() while true do end end) end end)
  while true do coroutine.resume(co) end
end)
pv.get('/gsub', function()
  while true do pcall(function() ('x'):gsub('.', function() while true do end end) end) end
end)
pv.get('/recurse', function()
  local function f(n) return f(n + 1) + 1 end
  return pv.text(tostring(pcall(f, 0)))
end)
"#
        ),
        &[],
    );
    let handler = handler_for(&root);
    // Unbounded recursion is in the list too: Lua's own stack limit is a million slots,
    // which is more instructions than the budget, so the budget is what stops it — and
    // the `pcall` around it does not get to catch anything.
    for route in [
        "/loop",
        "/pcall",
        "/aligned",
        "/coroutine",
        "/resume",
        "/gsub",
        "/recurse",
    ] {
        let response = handler.handle(get(&format!("/a/spin{route}"))).await;
        assert_eq!(
            response.status(),
            StatusCode::INTERNAL_SERVER_ERROR,
            "{route}"
        );
        let text = body_of(response).await;
        assert!(text.contains("instructions limit"), "{route}: {text}");
        assert!(text.contains("GET"), "{route}: {text}");
        let ok = handler.handle(get("/a/spin/ok")).await;
        assert_eq!(ok.status(), StatusCode::OK, "after {route}");
        assert_eq!(body_of(ok).await, "ok");
    }

    let audits = limit_audits(&handler);
    assert_eq!(audits.len(), 7, "{audits:?}");
    for audit in &audits {
        assert_eq!(audit["subject"], "spin");
        assert_eq!(audit["severity"], "warn");
        let detail: Value = serde_json::from_str(audit["detail"].as_str().unwrap()).unwrap();
        assert_eq!(detail["limit"], "instructions", "{detail}");
        assert!(detail["route"].as_str().unwrap().starts_with("GET /"));
    }
}

/// `spec/lua-api.md §5`, R2 — an app that allocates in a tight loop hits the memory limit:
/// a 500, a `lua.limit_exceeded` row, the VM replaced, and the next request healthy. An
/// allocation failure inside the hook path is the case R2 names; the loop below keeps the
/// hook firing while the allocator refuses.
#[tokio::test]
async fn test_spec_lua_5_memory_limit_aborts() {
    let root = tempfile::tempdir().unwrap();
    configure(
        &root,
        "[lua]\npool_size = 2\nmax_instructions = 5000000000\nmax_memory_mb = 4\nmax_seconds = 30\n",
    );
    app(
        &root,
        "eat",
        &format!(
            r#"
local pv = require 'privatium'
{OK_ROUTE}
hoard = {{}}
pv.get('/eat', function()
  while true do hoard[#hoard + 1] = string.rep('x', 4096) end
end)
pv.get('/caught', function()
  -- The allocator's refusal is catchable; an app that recovers from it has not exceeded
  -- the limit, and its next allocation is refused again if it has not let go.
  local ok, err = pcall(function() while true do hoard[#hoard + 1] = string.rep('x', 4096) end end)
  hoard = {{}}
  collectgarbage()
  return pv.text(tostring(ok) .. ' ' .. tostring(err))
end)
"#
        ),
        &[],
    );
    let handler = handler_for(&root);
    let eat = handler.handle(get("/a/eat/eat")).await;
    assert_eq!(eat.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let text = body_of(eat).await;
    assert!(text.contains("memory limit"), "{text}");
    let ok = handler.handle(get("/a/eat/ok")).await;
    assert_eq!(ok.status(), StatusCode::OK);
    let caught = handler.handle(get("/a/eat/caught")).await;
    assert_eq!(caught.status(), StatusCode::OK);
    let text = body_of(caught).await;
    assert!(text.starts_with("false"), "{text}");
    assert!(text.contains("memory"), "{text}");

    let audits = limit_audits(&handler);
    assert_eq!(audits.len(), 1, "{audits:?}");
    let detail: Value = serde_json::from_str(audits[0]["detail"].as_str().unwrap()).unwrap();
    assert_eq!(detail["limit"], "memory");
}

/// `spec/lua-api.md §5` — the wall clock aborts a loop the instruction count would not,
/// and a long SQL statement running in Rust, where the hook cannot fire, through the
/// connection's progress handler.
#[tokio::test]
async fn test_spec_lua_5_wallclock_limit_aborts() {
    let root = tempfile::tempdir().unwrap();
    configure(
        &root,
        "[lua]\npool_size = 2\nmax_instructions = 1000000000000\nmax_memory_mb = 32\nmax_seconds = 1\n",
    );
    app(
        &root,
        "slow",
        &format!(
            r#"
local pv = require 'privatium'
{OK_ROUTE}
pv.get('/loop', function() while true do end end)
pv.get('/sql', function()
  local ok, err = pcall(pv.query, [[
    WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c) SELECT count(*) AS n FROM c
  ]])
  -- The statement was interrupted; the handler may catch that, and the request still fails.
  return pv.text('caught: ' .. tostring(err))
end)
"#
        ),
        &[],
    );
    let handler = handler_for(&root);
    for route in ["/loop", "/sql"] {
        let started = std::time::Instant::now();
        let response = handler.handle(get(&format!("/a/slow{route}"))).await;
        let took = started.elapsed();
        assert_eq!(
            response.status(),
            StatusCode::INTERNAL_SERVER_ERROR,
            "{route}"
        );
        let text = body_of(response).await;
        assert!(text.contains("seconds limit"), "{route}: {text}");
        assert!(took.as_secs() < 8, "{route}: {took:?}");
        let ok = handler.handle(get("/a/slow/ok")).await;
        assert_eq!(ok.status(), StatusCode::OK, "after {route}");
    }
    let audits = limit_audits(&handler);
    assert_eq!(audits.len(), 2, "{audits:?}");
    for audit in &audits {
        let detail: Value = serde_json::from_str(audit["detail"].as_str().unwrap()).unwrap();
        assert_eq!(detail["limit"], "seconds", "{detail}");
    }
}

/// `spec/lua-api.md §5` — "It MUST NOT take down the node": after a limit trips, the
/// shell, another app, and the tripped app's other routes all answer.
#[tokio::test]
async fn test_spec_lua_5_limit_does_not_kill_node() {
    let root = tempfile::tempdir().unwrap();
    configure(
        &root,
        "[lua]\npool_size = 1\nmax_instructions = 200000\nmax_memory_mb = 16\nmax_seconds = 30\n",
    );
    app(
        &root,
        "bad",
        &format!(
            "local pv = require 'privatium'\n{OK_ROUTE}pv.get('/loop', function() while true do end end)\n"
        ),
        &[],
    );
    app(
        &root,
        "good",
        "local pv = require 'privatium'\npv.get('/', function() return 'fine' end)\n",
        &[],
    );
    let handler = handler_for(&root);
    for _ in 0..3 {
        let loop_ = handler.handle(get("/a/bad/loop")).await;
        assert_eq!(loop_.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
    // A pool of one: the only VM was discarded and rebuilt each time.
    assert_eq!(
        handler.handle(get("/a/bad/ok")).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        handler.handle(get("/a/good/")).await.status(),
        StatusCode::OK
    );
    assert_eq!(handler.handle(get("/")).await.status(), StatusCode::OK);
    let apps_page = body_of(handler.handle(get("/settings/apps")).await).await;
    assert!(apps_page.contains("id=\"app-bad\""), "{apps_page}");
    assert_eq!(limit_audits(&handler).len(), 3);
}

/// `spec/lua-api.md §5` — a global set while `app.lua` loads is the VM's baseline; a global
/// assigned in a handler (or in a `lib/` module a handler calls) lasts one request and is
/// never seen by another, on this VM or the other; a table the baseline holds and a handler
/// mutates in place persists per VM and is not shared — the footgun the linter checks; and
/// the environment's metatable is out of reach.
#[tokio::test]
async fn test_spec_lua_5_globals_are_per_vm() {
    let root = tempfile::tempdir().unwrap();
    configure(&root, LUA_CONFIG);
    app(
        &root,
        "glob",
        r#"
local pv = require 'privatium'
counter = 0
greeting = 'hi'
cache = {}
pv.get('/count', function() counter = counter + 1; return pv.text(tostring(counter)) end)
pv.get('/greet', function(req)
  local before = greeting
  if req.query.set then greeting = req.query.set end
  return pv.text(before .. '/' .. greeting)
end)
pv.get('/cache', function() cache[#cache + 1] = true; return pv.text(tostring(#cache)) end)
pv.get('/lib', function() return pv.text(tostring(require('m').bump())) end)
pv.get('/meta', function()
  local ok = pcall(setmetatable, _G, {})
  return pv.text(type(getmetatable(_G)) .. '/' .. tostring(ok) .. '/' .. tostring(_G.counter))
end)
"#,
        &[(
            "lib/m.lua",
            "local M = {}\nfunction M.bump() hits = (hits or 0) + 1; return hits end\nreturn M\n",
        )],
    );
    let handler = handler_for(&root);
    let hit = |path: &str| {
        let path = format!("/a/glob{path}");
        let handler = &handler;
        async move { body_of(handler.handle(get(&path)).await).await }
    };
    let mut counts = Vec::new();
    for _ in 0..4 {
        counts.push(hit("/count").await);
    }
    assert_eq!(
        counts,
        ["1", "1", "1", "1"],
        "a handler's global lasts one request"
    );
    assert_eq!(hit("/greet?set=bye").await, "hi/bye");
    assert_eq!(
        hit("/greet").await,
        "hi/hi",
        "the baseline is back next request"
    );
    assert_eq!(hit("/greet").await, "hi/hi");
    let mut lib = Vec::new();
    for _ in 0..3 {
        lib.push(hit("/lib").await);
    }
    assert_eq!(
        lib,
        ["1", "1", "1"],
        "a lib module's globals are request-scoped too"
    );
    let mut cached = Vec::new();
    for _ in 0..4 {
        cached.push(hit("/cache").await);
    }
    assert_eq!(
        cached,
        ["1", "1", "2", "2"],
        "a baseline table mutated in place persists per VM and is not shared"
    );
    assert_eq!(hit("/meta").await, "string/false/0");
}

// ---------------------------------------------------------------------------------------
// §2.4 — the route index
// ---------------------------------------------------------------------------------------

/// `docs/plans/phase-1.md §2.4` — an `app.lua` whose route table differs between VMs
/// fails to load, naming the divergence. `math.random` is seeded per state from the
/// state's own address and the clock, so two VMs draw the same number from a range of
/// 10⁹ with probability 10⁻⁹; both fixtures below put that number into every pattern they
/// register, so a divergence is found by count or by pattern either way. (An earlier
/// version registered `n % 5 + 1` routes and nothing else, which agreed one time in five
/// — the CI flake on the first two commits.) The count-versus-pattern wording of the
/// message is pinned deterministically by the unit tests in `lua/mod.rs`.
#[test]
fn test_route_index_divergence_fails_load() {
    let root = tempfile::tempdir().unwrap();
    configure(&root, LUA_CONFIG);
    app(
        &root,
        "flaky",
        "local pv = require 'privatium'\npv.get('/r' .. math.random(1, 1000000000), function() end)\n",
        &[],
    );
    app(
        &root,
        "count",
        "local pv = require 'privatium'\nlocal n = math.random(1, 1000000000)\nfor i = 1, n % 5 + 1 do pv.get('/c' .. n .. '/' .. i, function() end) end\n",
        &[],
    );
    app(
        &root,
        "steady",
        "local pv = require 'privatium'\npv.get('/a', function() end)\npv.post('/b', function() end)\n",
        &[],
    );
    let (node, report) = open(&root, false);
    assert_eq!(report.loaded, ["steady"]);
    assert_eq!(report.failed.len(), 2, "{:?}", report.failed);
    for failure in &report.failed {
        assert_eq!(failure.stage, Stage::Tier, "{failure}");
        assert!(
            failure.reason.contains("non-deterministically"),
            "{failure}"
        );
        assert!(failure.reason.contains("VM 1"), "{failure}");
        assert!(failure.reason.contains("VM 0"), "{failure}");
        assert!(failure.reason.contains("§2.4"), "{failure}");
    }
    let flaky = report.failed.iter().find(|f| f.folder == "flaky").unwrap();
    assert!(flaky.reason.contains("GET /r"), "{flaky}");
    assert!(node.app("flaky").is_none());
    assert!(node.app("steady").is_some());
    let routes: Vec<(String, String)> = node
        .app("steady")
        .unwrap()
        .lua_host()
        .unwrap()
        .routes()
        .iter()
        .map(|r| (r.method.clone(), r.pattern.clone()))
        .collect();
    assert_eq!(
        routes,
        [
            ("GET".to_owned(), "/a".to_owned()),
            ("POST".to_owned(), "/b".to_owned())
        ]
    );
}

// ---------------------------------------------------------------------------------------
// §3.1 — routing through handle
// ---------------------------------------------------------------------------------------

/// A Tier 1 route answers through `handle` with the app's own `header_for(origin)` policy
/// and `no-store`; `:params` and the query are bound; a form is parsed; every return kind
/// of `§3.1`'s table has its status; an unknown path is a 404 and a wrong method a 405.
#[tokio::test]
async fn test_tier1_route_answers_with_app_csp() {
    let root = tempfile::tempdir().unwrap();
    configure(&root, LUA_CONFIG);
    app(
        &root,
        "route",
        r#"
local pv = require 'privatium'
pv.get('/', function(req) return '<h1>home ' .. req.method .. ' ' .. req.path .. '</h1>' end)
pv.get('/item/:id/:rest', function(req)
  return pv.json({ id = req.params.id, rest = req.params.rest, q = req.query, htmx = req.is_htmx,
                   ua = req.headers['user-agent'], device = req.device })
end)
pv.post('/form', function(req) return pv.json({ form = req.form, body = req.body }) end)
pv.route('PUT', '/put', function(req) return pv.text('put ' .. req.body) end)
pv.get('/none', function() return nil end)
pv.get('/redir', function() return pv.redirect(url('/item/1/x')) end)
pv.get('/bad', function() return 42 end)
pv.get('/boom', function() error('kaboom') end)
pv.get('/view', function() return pv.render('missing') end)
"#,
        &[],
    );
    let handler = handler_for(&root);
    let expected_csp = {
        let node = handler.node().lock().unwrap();
        node.app("route")
            .unwrap()
            .csp()
            .header_for("http://127.0.0.1:8420")
    };

    let home = handler.handle(get("/a/route/")).await;
    assert_eq!(home.status(), StatusCode::OK);
    assert_eq!(header(&home, &CONTENT_TYPE), "text/html; charset=utf-8");
    assert_eq!(header(&home, &CONTENT_SECURITY_POLICY), expected_csp);
    assert_eq!(header(&home, &CACHE_CONTROL), "no-store");
    assert_eq!(body_of(home).await, "<h1>home GET /</h1>");

    let mut item = get("/a/route/item/01J9/a%20b?x=1&y=two+words");
    item.headers_mut()
        .insert("hx-request", "true".parse().unwrap());
    item.headers_mut()
        .insert("user-agent", "test/1".parse().unwrap());
    let item = json_of(handler.handle(item).await).await;
    assert_eq!(item["id"], "01J9");
    assert_eq!(item["rest"], "a b");
    assert_eq!(item["q"]["x"], "1");
    assert_eq!(item["q"]["y"], "two words");
    assert_eq!(item["htmx"], true);
    assert_eq!(item["ua"], "test/1");
    let id = handler.node().lock().unwrap().id().as_str().to_owned();
    assert_eq!(item["device"], id, "req.device is this node in Phase 1");

    let form = json_of(
        handler
            .handle(post("/a/route/form", "name=Ada+Lovelace&n=1"))
            .await,
    )
    .await;
    assert_eq!(form["form"]["name"], "Ada Lovelace");
    assert_eq!(form["form"]["n"], "1");
    assert_eq!(form["body"], "name=Ada+Lovelace&n=1");

    let put = axum::http::Request::builder()
        .method(Method::PUT)
        .uri("/a/route/put")
        .body(Body::from("raw"))
        .unwrap();
    let put = handler.handle(put).await;
    assert_eq!(put.status(), StatusCode::OK);
    assert_eq!(header(&put, &CONTENT_TYPE), "text/plain; charset=utf-8");
    assert_eq!(body_of(put).await, "put raw");

    let none = handler.handle(get("/a/route/none")).await;
    assert_eq!(none.status(), StatusCode::NO_CONTENT);
    assert_eq!(header(&none, &CONTENT_SECURITY_POLICY), expected_csp);

    let redirect = handler.handle(get("/a/route/redir")).await;
    assert_eq!(redirect.status(), StatusCode::SEE_OTHER);
    assert_eq!(header(&redirect, &LOCATION), "/a/route/item/1/x");

    let head = handler.handle(request(Method::HEAD, "/a/route/")).await;
    assert_eq!(head.status(), StatusCode::OK);
    assert!(body_of(head).await.is_empty());

    let missing = handler.handle(get("/a/route/nowhere")).await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert_eq!(header(&missing, &CONTENT_SECURITY_POLICY), expected_csp);
    let wrong = handler.handle(get("/a/route/form")).await;
    assert_eq!(wrong.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(header(&wrong, &ALLOW), "POST");
    assert_eq!(header(&wrong, &CONTENT_SECURITY_POLICY), expected_csp);

    let bad = handler.handle(get("/a/route/bad")).await;
    assert_eq!(bad.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert!(
        body_of(bad)
            .await
            .contains("returned a value of type integer")
    );
    let boom = handler.handle(get("/a/route/boom")).await;
    assert_eq!(boom.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(header(&boom, &CONTENT_SECURITY_POLICY), expected_csp);
    let text = body_of(boom).await;
    assert!(text.contains("kaboom"), "{text}");
    assert!(
        text.contains("app.lua:"),
        "the traceback names the file: {text}"
    );
    // `pv.render` of a view that does not exist is the app's error, not a missing engine.
    let view = handler.handle(get("/a/route/view")).await;
    assert_eq!(view.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert!(body_of(view).await.contains("views/missing.lsp"));

    // A body past the Tier 1 limit is refused before a byte reaches Lua (R6).
    let huge = axum::http::Request::builder()
        .method(Method::POST)
        .uri("/a/route/form")
        .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(vec![b'a'; 65 * 1024]))
        .unwrap();
    assert_eq!(
        handler.handle(huge).await.status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
}

// ---------------------------------------------------------------------------------------
// §3.2 — reading
// ---------------------------------------------------------------------------------------

/// `spec/lua-api.md §3.2`, `spec/data-dictionary.md §2.1` — what SQLite holds, as Lua holds
/// it: a BIGINT is a Lua integer, exact past 2⁵³; a DECIMAL is a string; `count(*)` an
/// integer and `1.5` a float; a BOOLEAN a boolean and a JSON column decoded, through a view
/// too; a NULL an absent key.
#[tokio::test]
async fn test_spec_lua_3_2_result_typing() {
    let root = tempfile::tempdir().unwrap();
    configure(&root, LUA_CONFIG);
    app(
        &root,
        "typed",
        r#"
local pv = require 'privatium'
pv.get('/write', function()
  local id = pv.append('thing', { name = 'a', amount = '12.3', big = '9007199254740993',
                                  ok = true, tags = { 'x', 'y' }, filled_on = '2026-08-28' })
  pv.append('thing', { name = 'b', amount = '0.2', big = '2', ok = false, tags = {} })
  return pv.text(id)
end)
pv.get('/read', function()
  local r = pv.query1("SELECT *, count(*) OVER () AS n, (SELECT decimal_sum(amount) FROM thing) AS total, 1.5 AS f, NULL AS nada FROM thing WHERE name = 'a'")
  local v = pv.query1('SELECT b, a, count(*) AS n FROM v_thing')
  local rows = pv.query('SELECT name, big FROM thing WHERE big > ? ORDER BY name', {'1'})
  return pv.json({
    amount = r.amount, amount_t = type(r.amount),
    big = r.big, big_t = math.type(r.big),
    ok = r.ok, ok_t = type(r.ok),
    tags = r.tags, tags_t = type(r.tags), tag2 = r.tags[2],
    filled_on = r.filled_on,
    nothing_absent = r.nada == nil,
    n = r.n, n_t = math.type(r.n),
    total = r.total, total_t = type(r.total),
    f = r.f, f_t = math.type(r.f),
    view_b = v.b, view_b_t = math.type(v.b), view_a = v.a, view_n = math.type(v.n),
    rows = rows, first_big_t = math.type(rows[1].big),
    ok_b = pv.query1("SELECT ok FROM thing WHERE name = 'b'").ok,
  })
end)
"#,
        &[("schema.sql", TYPED_DDL)],
    );
    let handler = handler_for(&root);
    let id = body_of(handler.handle(get("/a/typed/write")).await).await;
    assert_eq!(id.len(), 26);
    let read = json_of(handler.handle(get("/a/typed/read")).await).await;
    assert_eq!(read["amount"], "12.30", "at the declared scale");
    assert_eq!(read["amount_t"], "string");
    assert_eq!(read["big"], 9_007_199_254_740_993_i64, "exact past 2^53");
    assert_eq!(read["big_t"], "integer");
    assert_eq!(read["ok"], true);
    assert_eq!(read["ok_t"], "boolean");
    assert_eq!(read["ok_b"], false);
    assert_eq!(read["tags"], serde_json::json!(["x", "y"]));
    assert_eq!(read["tags_t"], "table");
    assert_eq!(read["tag2"], "y");
    assert_eq!(read["filled_on"], "2026-08-28");
    assert_eq!(read["nothing_absent"], true);
    assert_eq!(read["n"], 1, "count(*) is a Lua integer");
    assert_eq!(read["n_t"], "integer");
    assert_eq!(
        read["total"], "12.50",
        "decimal_sum is text and stays a string"
    );
    assert_eq!(read["total_t"], "string");
    assert_eq!(read["f"], 1.5);
    assert_eq!(read["f_t"], "float");
    assert_eq!(read["view_b"], 9_007_199_254_740_993_i64, "through a view");
    assert_eq!(read["view_b_t"], "integer");
    assert_eq!(
        read["view_a"], "12.30",
        "a DECIMAL stays a string through a view"
    );
    assert_eq!(read["view_n"], "integer");
    assert_eq!(read["first_big_t"], "integer");
    assert_eq!(read["rows"][0]["name"], "a");
    assert_eq!(read["rows"][1]["big"], 2);
}

/// `spec/app-contract.md §7` — a `pv.query` that writes is a Lua error the handler can
/// catch, never a node failure: the table is untouched and the next request answers.
#[tokio::test]
async fn test_spec_app_7_pv_query_cannot_write() {
    let root = tempfile::tempdir().unwrap();
    configure(&root, LUA_CONFIG);
    app(
        &root,
        "ro",
        r#"
local pv = require 'privatium'
pv.get('/write', function()
  local results = {}
  for _, sql in ipairs({
    "INSERT INTO thing (id, name) VALUES ('x', 'y')",
    "UPDATE thing SET name = 'z'",
    "DELETE FROM thing",
    "DROP TABLE thing",
    "PRAGMA query_only = 0",
    "ATTACH 'leak.sqlite' AS leak",
    "SELECT load_extension('x')",
  }) do
    local ok, err = pcall(pv.query, sql)
    results[#results + 1] = { ok = ok, err = tostring(err) }
  end
  return pv.json({ results = results, count = pv.query1('SELECT count(*) AS n FROM thing').n })
end)
pv.get('/count', function() return pv.json({ count = pv.query1('SELECT count(*) AS n FROM thing').n }) end)
"#,
        &[("schema.sql", TYPED_DDL)],
    );
    let handler = handler_for(&root);
    let write = json_of(handler.handle(get("/a/ro/write")).await).await;
    for result in write["results"].as_array().unwrap() {
        assert_eq!(result["ok"], false, "{result}");
        let err = result["err"].as_str().unwrap();
        assert!(err.contains("pv.query: "), "{err}");
        assert!(
            err.contains("not authorized") || err.contains("readonly") || err.contains("read-only"),
            "{err}"
        );
    }
    assert_eq!(write["count"], 0);
    let count = json_of(handler.handle(get("/a/ro/count")).await).await;
    assert_eq!(count["count"], 0);
}

/// `spec/lua-api.md §3.2` — `pv.dec` is exact: 0.1 + 0.2 is 0.3; `/` divides at the larger
/// scale of the operands and `a:div(b, scale)` at a named one, both rounding half away from
/// zero; a float, an overflow and a zero divisor are errors rather than wrong numbers.
#[tokio::test]
async fn test_spec_lua_3_2_pv_dec_is_exact() {
    let root = tempfile::tempdir().unwrap();
    configure(&root, LUA_CONFIG);
    app(
        &root,
        "dec",
        r#"
local pv = require 'privatium'
local function err(f, ...) local ok, e = pcall(f, ...) return (not ok) and tostring(e) or 'no error' end
pv.get('/', function()
  local a, b = pv.dec('0.1'), pv.dec('0.2')
  return pv.json({
    sum = tostring(a + b),
    sum_is = (a + b) == pv.dec('0.3'),
    mixed = tostring(pv.dec('12.34') + '0.66'),
    int = tostring(pv.dec(3) * pv.dec('1.10')),
    sub = tostring(pv.dec('10.00') - pv.dec('9.50')),
    neg = tostring(-pv.dec('1.5')),
    lt = pv.dec('9.50') < pv.dec('10'),
    le = pv.dec('1.0') <= pv.dec('1'),
    eq = pv.dec('1.0') == pv.dec('1'),
    div = tostring(pv.dec('1'):div(pv.dec('3'), 4)),
    div2 = tostring(pv.dec('10.00'):div('4', 2)),
    div_round = tostring(pv.dec('2.5'):div('1', 0)),
    div_round_neg = tostring(pv.dec('-2.5'):div('1', 0)),
    div_string = tostring(pv.dec('7'):div(2, 1)),
    scale = pv.dec('12.345'):with_scale(2):tostring(),
    scale_up = tostring(pv.dec('1'):with_scale(3)),
    money = fmt.money('1234567.5'),
    money_dec = fmt.money(pv.dec('-0.5')),
    slash = tostring(a / b),
    slash_scale = tostring(pv.dec('10.00') / 3),
    slash_round = tostring(pv.dec('2') / pv.dec('3')),
    slash_zero = err(function() return a / pv.dec('0') end),
    float = err(pv.dec, 0.1),
    text = err(pv.dec, 'abc'),
    zero = err(function() return a:div(pv.dec('0'), 2) end),
    overflow = err(function() local big = pv.dec('1' .. string.rep('0', 35)); return big * big end),
  })
end)
"#,
        &[],
    );
    let handler = handler_for(&root);
    let dec = json_of(handler.handle(get("/a/dec/")).await).await;
    assert_eq!(dec["sum"], "0.3");
    assert_eq!(dec["sum_is"], true);
    assert_eq!(dec["mixed"], "13.00");
    assert_eq!(dec["int"], "3.30");
    assert_eq!(dec["sub"], "0.50");
    assert_eq!(dec["neg"], "-1.5");
    assert_eq!(dec["lt"], true);
    assert_eq!(dec["le"], true);
    assert_eq!(dec["eq"], true);
    assert_eq!(dec["div"], "0.3333");
    assert_eq!(dec["div2"], "2.50");
    assert_eq!(dec["div_round"], "3", "half away from zero");
    assert_eq!(dec["div_round_neg"], "-3");
    assert_eq!(dec["div_string"], "3.5");
    assert_eq!(dec["scale"], "12.35");
    assert_eq!(dec["scale_up"], "1.000");
    assert_eq!(dec["money"], "1,234,567.50");
    assert_eq!(dec["money_dec"], "-0.50");
    assert_eq!(dec["slash"], "0.5", "0.1 / 0.2 at the larger scale");
    assert_eq!(dec["slash_scale"], "3.33", "10.00 / 3 keeps two places");
    assert_eq!(
        dec["slash_round"], "1",
        "2 / 3 at scale 0 rounds half away from zero"
    );
    for (key, expected) in [
        ("slash_zero", "division by zero"),
        ("float", "not exact"),
        ("text", "not a decimal"),
        ("zero", "division by zero"),
        ("overflow", "does not fit"),
    ] {
        assert!(
            dec[key].as_str().unwrap().contains(expected),
            "{key}: {}",
            dec[key]
        );
    }
}

// ---------------------------------------------------------------------------------------
// §3.3 — writing
// ---------------------------------------------------------------------------------------

/// `spec/lua-api.md §3.3` — both arities of `pv.append`, `nil` as the id in the
/// three-argument form, the returned id, `pv.delete` as a tombstone `get_row` no longer
/// finds, and a call in neither form as a Lua error.
#[tokio::test]
async fn test_spec_lua_3_append_arities() {
    let root = tempfile::tempdir().unwrap();
    configure(&root, LUA_CONFIG);
    app(
        &root,
        "arity",
        r#"
local pv = require 'privatium'
pv.get('/two', function() return pv.text(pv.append('thing', { name = 'two' })) end)
pv.get('/three', function() return pv.text(pv.append('thing', 'fixed', { name = 'three', big = '7' })) end)
pv.get('/amend', function() return pv.text(pv.append('thing', 'fixed', { name = 'amended' })) end)
pv.get('/nil', function() return pv.text(pv.append('thing', nil, { name = 'nil' })) end)
pv.get('/row/:id', function(req) return pv.json({ row = pv.get_row('thing', req.params.id) }) end)
pv.get('/delete/:id', function(req) pv.delete('thing', req.params.id) return pv.text('gone') end)
pv.get('/bad', function()
  local a = { pcall(pv.append, 'thing', 'x') }
  local b = { pcall(pv.append, 'thing', 'x', 'y') }
  local c = { pcall(pv.append, 'bad name', {}) }
  local d = { pcall(pv.append, 'thing', { [1] = 'positional' }) }
  return pv.json({ a = tostring(a[2]), b = tostring(b[2]), c = tostring(c[2]), d = tostring(d[2]) })
end)
"#,
        &[("schema.sql", TYPED_DDL)],
    );
    let handler = handler_for(&root);
    let log = log_path(&handler, "arity");

    let two = body_of(handler.handle(get("/a/arity/two")).await).await;
    assert_eq!(two.len(), 26, "a minted ULID: {two}");
    let three = body_of(handler.handle(get("/a/arity/three")).await).await;
    assert_eq!(three, "fixed");
    let amended = body_of(handler.handle(get("/a/arity/amend")).await).await;
    assert_eq!(amended, "fixed");
    let nil = body_of(handler.handle(get("/a/arity/nil")).await).await;
    assert_eq!(nil.len(), 26);
    assert_ne!(nil, two);

    let lines = log_lines(&log);
    assert_eq!(lines.len(), 4);
    assert_eq!(lines[0]["id"], two);
    assert_eq!(lines[0]["d"]["name"], "two");
    assert_eq!(lines[1]["id"], "fixed");
    assert_eq!(lines[1]["d"]["big"], "7", "a BIGINT crosses as a string");
    assert_eq!(lines[2]["id"], "fixed");
    assert_eq!(lines[3]["id"], nil);
    let seqs: Vec<u64> = lines.iter().map(|l| l["seq"].as_u64().unwrap()).collect();
    assert_eq!(seqs, [1, 2, 3, 4]);

    let row = json_of(handler.handle(get("/a/arity/row/fixed")).await).await;
    assert_eq!(
        row["row"]["name"], "amended",
        "an amendment, not a second row"
    );
    assert!(
        row["row"].get("big").is_none(),
        "the amendment carried no big: {row}"
    );
    let count: i64 = {
        let node = handler.node().lock().unwrap();
        node.app("arity")
            .unwrap()
            .store()
            .app_conn()
            .unwrap()
            .query_row("SELECT count(*) FROM thing", [], |r| r.get(0))
            .unwrap()
    };
    assert_eq!(count, 3);

    assert_eq!(
        body_of(handler.handle(get("/a/arity/delete/fixed")).await).await,
        "gone"
    );
    let gone = json_of(handler.handle(get("/a/arity/row/fixed")).await).await;
    assert!(gone.get("row").is_none(), "a tombstoned id is nil: {gone}");
    assert_eq!(log_lines(&log)[4]["op"], "del");
    {
        let node = handler.node().lock().unwrap();
        assert!(
            node.app("arity")
                .unwrap()
                .store()
                .is_tombstoned("thing", "fixed")
                .unwrap()
        );
    }

    let bad = json_of(handler.handle(get("/a/arity/bad")).await).await;
    for key in ["a", "b"] {
        assert!(
            bad[key].as_str().unwrap().contains("append(tbl, data)"),
            "{key}: {}",
            bad[key]
        );
    }
    assert!(bad["c"].as_str().unwrap().contains("not a table name"));
    assert!(bad["d"].as_str().unwrap().contains("named by strings"));
    assert_eq!(log_lines(&log).len(), 5, "nothing bad reached the log");
}

/// `spec/lua-api.md §3.3` — `tx.append` returns the ULID before the batch is written so a
/// later event can reference it; the batch lands with one `ts` and contiguous `seq`; an
/// error inside the function means none of it lands; `pv.append` inside a batch is refused.
#[tokio::test]
async fn test_spec_lua_3_batch_all_or_nothing() {
    let root = tempfile::tempdir().unwrap();
    configure(&root, LUA_CONFIG);
    app(
        &root,
        "batch",
        r#"
local pv = require 'privatium'
pv.get('/ok', function()
  local seen
  pv.batch(function(tx)
    local a = tx.append('thing', { name = 'leaf' })
    seen = a
    tx.append('thing', 'q1', { name = 'question', tags = { a } })
    tx.delete('thing', 'cursor')
    -- Not visible yet: the batch has not been written.
    assert(pv.get_row('thing', a) == nil, 'visible before the batch was written')
  end)
  local row = pv.get_row('thing', 'q1')
  return pv.json({ a = seen, referenced = row.tags[1], leaf = pv.get_row('thing', seen).name })
end)
pv.get('/fail', function()
  local ok, err = pcall(pv.batch, function(tx)
    tx.append('thing', { name = 'never' })
    error('boom')
  end)
  return pv.json({ ok = ok, err = tostring(err) })
end)
pv.get('/mixed', function()
  local ok, err = pcall(pv.batch, function(tx)
    tx.append('thing', { name = 'never' })
    pv.append('thing', { name = 'never either' })
  end)
  return pv.json({ ok = ok, err = tostring(err) })
end)
pv.get('/nested', function()
  local ok, err = pcall(pv.batch, function(tx) pv.batch(function() end) end)
  return pv.json({ ok = ok, err = tostring(err) })
end)
pv.get('/stale', function()
  local stale
  pv.batch(function(tx) stale = tx end)
  local ok, err = pcall(stale.append, 'thing', { name = 'late' })
  return pv.json({ ok = ok, err = tostring(err) })
end)
pv.get('/empty', function() pv.batch(function() end) return pv.text('nothing') end)
"#,
        &[("schema.sql", TYPED_DDL)],
    );
    let handler = handler_for(&root);
    let log = log_path(&handler, "batch");

    let ok = json_of(handler.handle(get("/a/batch/ok")).await).await;
    let a = ok["a"].as_str().unwrap().to_owned();
    assert_eq!(a.len(), 26);
    assert_eq!(
        ok["referenced"], a,
        "the later event referenced the minted id"
    );
    assert_eq!(ok["leaf"], "leaf");
    let lines = log_lines(&log);
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0]["id"], a);
    assert_eq!(lines[1]["id"], "q1");
    assert_eq!(lines[2]["op"], "del");
    assert_eq!(lines[2]["id"], "cursor");
    assert!(lines[2].get("d").is_none());
    let seqs: Vec<u64> = lines.iter().map(|l| l["seq"].as_u64().unwrap()).collect();
    assert_eq!(seqs, [1, 2, 3], "contiguous");
    let lams: Vec<u64> = lines.iter().map(|l| l["lam"].as_u64().unwrap()).collect();
    assert_eq!(lams, [1, 2, 3]);
    assert_eq!(lines[0]["ts"], lines[2]["ts"], "one ts");

    for (route, expected) in [
        ("/fail", "boom"),
        ("/mixed", "use tx.append"),
        ("/nested", "does not nest"),
        ("/stale", "only valid inside"),
    ] {
        let refused = json_of(handler.handle(get(&format!("/a/batch{route}"))).await).await;
        assert_eq!(refused["ok"], false, "{route}: {refused}");
        assert!(
            refused["err"].as_str().unwrap().contains(expected),
            "{route}: {refused}"
        );
        assert_eq!(log_lines(&log).len(), 3, "{route} reached the log");
    }
    assert_eq!(
        body_of(handler.handle(get("/a/batch/empty")).await).await,
        "nothing"
    );
    assert_eq!(log_lines(&log).len(), 3);
}

/// `spec/lua-api.md §3.3`, `spec/data-dictionary.md §2.1` — typed writes: a Lua integer for
/// a BIGINT and a number for a DECIMAL land as digits, `'yes'` lands as a boolean, and a
/// date, time or timestamp in any accepted spelling lands in the ISO spelling, with
/// `ui.date_format` deciding whether `3/9` is March or September; a value that is not its
/// type refuses the append naming the column, and nothing reaches the log.
#[tokio::test]
async fn test_spec_lua_3_3_typed_writes_normalize() {
    let root = tempfile::tempdir().unwrap();
    configure(&root, LUA_CONFIG);
    app(
        &root,
        "typedw",
        r#"
local pv = require 'privatium'
pv.get('/write', function(req)
  local id = pv.append('thing', {
    name = 'a', amount = 12.5, big = 42, ok = 'yes',
    filled_on = req.query.date, seen_at = '2026-09-03 14:03', at_time = '2:30 pm',
  })
  return pv.json({ id = id, row = pv.get_row('thing', id) })
end)
pv.get('/bad', function(req)
  local ok, err = pcall(pv.append, 'thing', { [req.query.col] = req.query.val })
  return pv.json({ ok = ok, err = tostring(err) })
end)
"#,
        &[(
            "schema.sql",
            "CREATE TABLE thing (id VARCHAR PRIMARY KEY, name VARCHAR, amount DECIMAL(18,2), \
             big BIGINT, ok BOOLEAN, filled_on DATE, seen_at TIMESTAMPTZ, at_time TIME);",
        )],
    );
    let handler = handler_for(&root);
    let log = log_path(&handler, "typedw");
    let encode = |text: &str| text.replace(' ', "+").replace('/', "%2F");

    let written = json_of(
        handler
            .handle(get(&format!("/a/typedw/write?date={}", encode("3/9/2026"))))
            .await,
    )
    .await;
    let line = log_lines(&log).pop().unwrap();
    assert_eq!(
        line["d"]["amount"], "12.50",
        "a Lua number lands as digits at scale"
    );
    assert_eq!(
        line["d"]["big"], "42",
        "a Lua integer lands as a string (§2.1)"
    );
    assert_eq!(line["d"]["ok"], true);
    assert_eq!(
        line["d"]["filled_on"], "2026-03-09",
        "month first by default"
    );
    assert_eq!(line["d"]["seen_at"], "2026-09-03T14:03:00.000Z");
    assert_eq!(line["d"]["at_time"], "14:30:00");
    assert_eq!(written["row"]["amount"], "12.50");
    assert_eq!(written["row"]["big"], 42);
    assert_eq!(written["row"]["ok"], true);
    assert_eq!(written["row"]["filled_on"], "2026-03-09");

    for (typed, expected) in [
        ("2026-09-03", "2026-09-03"),
        ("September 3, 2026", "2026-09-03"),
        ("3 Sep 2026", "2026-09-03"),
        ("03-SEP-26", "2026-09-03"),
        ("20260903", "2026-09-03"),
        ("31/12/2026", "2026-12-31"),
        ("2026-09-03T10:00:00Z", "2026-09-03"),
    ] {
        let written = json_of(
            handler
                .handle(get(&format!("/a/typedw/write?date={}", encode(typed))))
                .await,
        )
        .await;
        assert_eq!(written["row"]["filled_on"], expected, "{typed}");
    }
    let before = log_lines(&log).len();

    for (column, value, expected) in [
        ("amount", "abc", "not a decimal"),
        ("amount", "12.345", "more than 2 decimal"),
        ("big", "1.5", "not an integer"),
        ("ok", "maybe", "not a boolean"),
        ("filled_on", "yesterday", "not a date"),
        ("seen_at", "soon", "not a timestamp"),
        ("at_time", "noon", "not a time"),
    ] {
        let bad = json_of(
            handler
                .handle(get(&format!(
                    "/a/typedw/bad?col={column}&val={}",
                    encode(value)
                )))
                .await,
        )
        .await;
        assert_eq!(bad["ok"], false, "{column}={value}: {bad}");
        let err = bad["err"].as_str().unwrap();
        assert!(err.contains(&format!("thing.{column}")), "{err}");
        assert!(err.contains(expected), "{err}");
    }
    assert_eq!(
        log_lines(&log).len(),
        before,
        "nothing refused reached the log"
    );

    // `ui.date_format = "eu"` reads the day first where the ranges do not decide.
    {
        let mut node = handler.node().lock().unwrap();
        let at = privatium_core::log::now();
        node.sys_log_mut()
            .put(
                "sys_setting",
                "ui.date_format",
                &serde_json::json!({ "value": "\"eu\"", "updated_at": at }),
            )
            .unwrap();
        node.refresh().unwrap();
    }
    let written = json_of(
        handler
            .handle(get(&format!("/a/typedw/write?date={}", encode("3/9/2026"))))
            .await,
    )
    .await;
    assert_eq!(
        written["row"]["filled_on"], "2026-09-03",
        "day first under eu"
    );
}

/// `spec/lua-api.md §3.4` — `pv.on('append')` fires for this node's own appends: a
/// handler's, and the owner's seed. The reaction is itself an append, and lands.
#[tokio::test]
async fn test_seed_and_own_appends_fire_pv_on() {
    let root = tempfile::tempdir().unwrap();
    configure(&root, LUA_CONFIG);
    let apps = Node::open(root.path()).unwrap().paths().apps_dir();
    write_app(
        &apps,
        "react",
        Some(&lua_manifest("react")),
        &[
            (
                "app.lua",
                r#"
local pv = require 'privatium'
pv.on('append', function(ev)
  if ev.tbl == 'thing' and ev.op == 'put' then
    pv.append('echo', ev.id, { seq = tostring(ev.seq), lam = tostring(ev.lam), ts = ev.ts,
                               dev = ev.dev, app = ev.app, name = ev.d.name })
  end
end)
pv.get('/add', function() return pv.text(pv.append('thing', { name = 'added' })) end)
"#,
            ),
            (
                "schema.sql",
                "CREATE TABLE thing (id VARCHAR PRIMARY KEY, name VARCHAR);\n\
                 CREATE TABLE echo (id VARCHAR PRIMARY KEY, seq BIGINT, lam BIGINT, ts TIMESTAMPTZ, dev VARCHAR, app VARCHAR, name VARCHAR);",
            ),
            (
                "sample/seed.jsonl",
                "{\"op\":\"put\",\"tbl\":\"thing\",\"id\":\"a\",\"d\":{\"name\":\"Ada\"}}\n\
                 {\"op\":\"put\",\"tbl\":\"thing\",\"id\":\"b\",\"d\":{\"name\":\"Grace\"}}\n\
                 {\"op\":\"del\",\"tbl\":\"thing\",\"id\":\"b\"}\n",
            ),
        ],
    );
    let handler = handler_for(&root);
    let log = log_path(&handler, "react");
    let dev = handler.node().lock().unwrap().id().as_str().to_owned();

    let action = "/settings/apps/react/seed";
    let token = handler.csrf().token(action);
    let seeded = handler
        .handle(post(action, &format!("_csrf={token}")))
        .await;
    assert_eq!(seeded.status(), StatusCode::SEE_OTHER);
    let lines = log_lines(&log);
    assert_eq!(
        lines.len(),
        5,
        "three seed events, two reactions: {lines:?}"
    );
    assert_eq!(lines[3]["tbl"], "echo");
    assert_eq!(lines[3]["id"], "a");
    assert_eq!(lines[3]["d"]["seq"], "1");
    assert_eq!(lines[3]["d"]["lam"], "1");
    assert_eq!(lines[3]["d"]["ts"], lines[0]["ts"]);
    assert_eq!(lines[3]["d"]["dev"], dev);
    assert_eq!(lines[3]["d"]["app"], "react");
    assert_eq!(lines[3]["d"]["name"], "Ada");
    assert_eq!(lines[4]["id"], "b");
    assert_eq!(lines[4]["d"]["seq"], "2");

    let added = body_of(handler.handle(get("/a/react/add")).await).await;
    let lines = log_lines(&log);
    assert_eq!(lines.len(), 7);
    assert_eq!(lines[5]["id"], added);
    assert_eq!(lines[6]["tbl"], "echo");
    assert_eq!(lines[6]["id"], added);
    assert_eq!(lines[6]["d"]["seq"], "6");
    assert_eq!(lines[6]["d"]["name"], "added");
}

// ---------------------------------------------------------------------------------------
// §4.0 — url, and the mount
// ---------------------------------------------------------------------------------------

/// `spec/lua-api.md §4.0` — `url` and `pv.url` are `wire::url(mount, path)`, differing
/// between host and solo mode there and nowhere else; a hard-coded `/a/<slug>/` is passed
/// through untouched by everything, which is exactly why it breaks in solo mode.
#[tokio::test]
async fn test_url_is_the_mount_and_nothing_rewrites_a_literal() {
    let src = r#"
local pv = require 'privatium'
pv.get('/urls', function() return pv.json({ url = url('/x/y'), pv_url = pv.url('x'), root = url('/'), root2 = url('') }) end)
pv.get('/literal', function() return pv.redirect('/a/other/x') end)
pv.get('/settings', function() return 'shadowed' end)
pv.get('/api/v1/health', function() return 'shadowed' end)
pv.get('/static/x.js', function() return 'shadowed' end)
pv.get('/play', function() return 'mine' end)
"#;
    let host = tempfile::tempdir().unwrap();
    configure(&host, LUA_CONFIG);
    app(&host, "urls", src, &[]);
    let handler = handler_for(&host);
    let urls = json_of(handler.handle(get("/a/urls/urls")).await).await;
    assert_eq!(urls["url"], "/a/urls/x/y");
    assert_eq!(urls["pv_url"], "/a/urls/x");
    assert_eq!(urls["root"], "/a/urls/");
    assert_eq!(urls["root2"], "/a/urls/");
    let literal = handler.handle(get("/a/urls/literal")).await;
    assert_eq!(header(&literal, &LOCATION), "/a/other/x");
    // In host mode nothing is shadowed: the routes live under the mount.
    assert!(
        !handler
            .report()
            .warnings
            .iter()
            .any(|w| matches!(w, Warning::RouteShadowed { .. }))
    );
    assert_eq!(
        body_of(handler.handle(get("/a/urls/settings")).await).await,
        "shadowed"
    );

    let solo = tempfile::tempdir().unwrap();
    configure(
        &solo,
        &format!("[node]\nmode = \"solo\"\napp = \"urls\"\n{LUA_CONFIG}"),
    );
    app(&solo, "urls", src, &[]);
    let handler = handler_for(&solo);
    let urls = json_of(handler.handle(get("/urls")).await).await;
    assert_eq!(urls["url"], "/x/y");
    assert_eq!(urls["pv_url"], "/x");
    assert_eq!(urls["root"], "/");
    let literal = handler.handle(get("/literal")).await;
    assert_eq!(header(&literal, &LOCATION), "/a/other/x");
    assert_eq!(body_of(handler.handle(get("/play")).await).await, "mine");

    // `spec/protocol.md §9.1` — the framework wins at request time, and said so at load.
    let settings = body_of(handler.handle(get("/settings")).await).await;
    assert!(settings.contains("Settings"), "{settings}");
    assert!(!settings.contains("shadowed"), "{settings}");
    let health = body_of(handler.handle(get("/api/v1/health")).await).await;
    assert!(health.starts_with("{\"v\":1"), "{health}");
    assert_eq!(
        handler.handle(get("/static/x.js")).await.status(),
        StatusCode::NOT_FOUND
    );
    let shadowed: Vec<(String, &str)> = handler
        .report()
        .warnings
        .iter()
        .filter_map(|w| match w {
            Warning::RouteShadowed {
                slug,
                route,
                prefix,
            } if slug == "urls" => Some((route.clone(), *prefix)),
            _ => None,
        })
        .collect();
    assert_eq!(
        shadowed,
        [
            ("/settings".to_owned(), "/settings"),
            ("/api/v1/health".to_owned(), "/api"),
            ("/static/x.js".to_owned(), "/static"),
        ]
    );
    let text = handler.report().warnings[0].to_string();
    assert!(
        text.contains("route /settings is shadowed by the framework prefix /settings"),
        "{text}"
    );
}

/// `spec/lua-api.md §3.4` — `pv.node()`, `pv.device()`, `pv.setting` with its default,
/// `pv.log` to the diagnostic log, and `fmt.*` under the `ui.*` settings.
#[tokio::test]
async fn test_spec_lua_3_4_node_facts_and_settings() {
    let root = tempfile::tempdir().unwrap();
    configure(&root, LUA_CONFIG);
    app(
        &root,
        "facts",
        r#"
local pv = require 'privatium'
pv.get('/', function()
  local n = pv.node()
  pv.log('info', 'a line for the diagnostic log')
  print('and', 'another', 1)
  local bad = { pcall(pv.log, 'loud', 'x') }
  return pv.json({
    id = n.id, name = n.name, solo = n.solo, peers = n.peers, tier = n.restore_tier,
    device = pv.device(),
    unset = pv.setting('no.such.key', 'fallback'),
    unset_nil = pv.setting('no.such.key') == nil,
    retention = pv.setting('snapshot.retention_days', 0),
    date_iso = fmt.date('2026-08-28'), date_ts = fmt.date('2026-08-28T14:03:11.412Z'),
    date_bad = fmt.date('yesterday'), rel = fmt.rel(pv.now()), rel_bad = fmt.rel('soon'),
    t = t('greeting'),
    icon = icon('trash'), icon_labeled = icon('trash', 'Delete this fill'),
    icon_bad = tostring(select(2, pcall(icon, 'trash', { label = 'x' }))),
    bad_level = tostring(bad[2]),
    ulid = #pv.ulid(), now = #pv.now(),
  })
end)
"#,
        &[],
    );
    let handler = handler_for(&root);
    let id = handler.node().lock().unwrap().id().as_str().to_owned();
    let facts = json_of(handler.handle(get("/a/facts/")).await).await;
    assert_eq!(facts["id"], id);
    assert_eq!(facts["name"], id, "no display name yet, so the Node ID");
    assert_eq!(facts["solo"], false);
    assert_eq!(facts["peers"], 0);
    assert_eq!(facts["tier"], 3, "a fresh app is built by replay");
    assert_eq!(facts["device"], id);
    assert_eq!(facts["unset"], "fallback");
    assert_eq!(facts["unset_nil"], true);
    assert_eq!(facts["retention"], 0, "unset, so the caller's default");
    assert_eq!(facts["date_iso"], "2026-08-28");
    assert_eq!(facts["date_ts"], "2026-08-28");
    assert_eq!(facts["date_bad"], "yesterday");
    assert_eq!(facts["rel"], "just now");
    assert_eq!(facts["rel_bad"], "soon");
    assert_eq!(facts["t"], "greeting");
    assert!(
        facts["icon"]
            .as_str()
            .unwrap()
            .contains("aria-hidden=\"true\"")
    );
    assert!(
        facts["icon_labeled"]
            .as_str()
            .unwrap()
            .contains("<title>Delete this fill</title>")
    );
    assert!(
        facts["icon_bad"]
            .as_str()
            .unwrap()
            .contains("label is a string")
    );
    assert!(facts["bad_level"].as_str().unwrap().contains("not one of"));
    assert_eq!(facts["ulid"], 26);
    assert_eq!(facts["now"], 24);

    // `ui.date_format` and `ui.locale` from sys_setting change fmt.* on the next request.
    {
        let mut node = handler.node().lock().unwrap();
        let at = privatium_core::log::now();
        node.sys_log_mut()
            .put(
                "sys_setting",
                "ui.date_format",
                &serde_json::json!({ "value": "\"eu\"", "updated_at": at }),
            )
            .unwrap();
        node.sys_log_mut()
            .put(
                "sys_setting",
                "ui.locale",
                &serde_json::json!({ "value": "\"de-DE\"", "updated_at": at }),
            )
            .unwrap();
        node.refresh().unwrap();
    }
    app(
        &root,
        "facts2",
        "local pv = require 'privatium'\npv.get('/', function() return pv.json({ date = fmt.date('2026-08-28'), money = fmt.money('1234.5'), setting = pv.setting('ui.locale', 'x') }) end)\n",
        &[],
    );
    let handler = handler_for(&root);
    let facts = json_of(handler.handle(get("/a/facts2/")).await).await;
    assert_eq!(facts["date"], "28/08/2026");
    assert_eq!(facts["money"], "1.234,50");
    assert_eq!(facts["setting"], "de-DE");
}

// ---------------------------------------------------------------------------------------
// The reference apps
// ---------------------------------------------------------------------------------------

/// `apps/hello` and `apps/animals` load, register their routes, and answer: their views
/// are a clear 503 until M8, their redirects and appends are real.
#[tokio::test]
async fn test_reference_apps_load_and_route() {
    let root = tempfile::tempdir().unwrap();
    configure(&root, LUA_CONFIG);
    let (node, report) = open(&root, true);
    assert_eq!(report.loaded, ["animals", "hello", "sketch"]);
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    let routes = |slug: &str| -> Vec<String> {
        node.app(slug)
            .unwrap()
            .lua_host()
            .unwrap()
            .routes()
            .iter()
            .map(|r| format!("{} {}", r.method, r.pattern))
            .collect()
    };
    assert_eq!(routes("hello"), ["GET /", "GET /edit", "POST /name"]);
    assert_eq!(
        routes("animals"),
        [
            "GET /",
            "POST /start",
            "POST /answer",
            "POST /seed",
            "GET /teach",
            "POST /teach",
            "GET /knowledge",
            "POST /reset",
        ]
    );
    let handler = Handler::new(node, report);
    let hello_log = log_path(&handler, "hello");
    let animals_log = log_path(&handler, "animals");

    for path in [
        "/a/hello/",
        "/a/hello/edit",
        "/a/animals/",
        "/a/animals/knowledge",
    ] {
        let response = handler.handle(get(path)).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE, "{path}");
        let text = body_of(response).await;
        assert!(text.contains("no LSP compiler"), "{path}: {text}");
    }
    assert_eq!(
        handler.handle(get("/a/animals/nope")).await.status(),
        StatusCode::NOT_FOUND
    );

    // hello: the name is stored and the app redirects home through url().
    let named = handler
        .handle(post("/a/hello/name", "display_name=+Gabriel+"))
        .await;
    assert_eq!(named.status(), StatusCode::SEE_OTHER);
    assert_eq!(header(&named, &LOCATION), "/a/hello/");
    let lines = log_lines(&hello_log);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["tbl"], "profile");
    assert_eq!(lines[0]["d"]["display_name"], "Gabriel");
    // A second name amends the same row (the app reuses `me.id`).
    let renamed = handler
        .handle(post("/a/hello/name", "display_name=Ada"))
        .await;
    assert_eq!(renamed.status(), StatusCode::SEE_OTHER);
    let lines = log_lines(&hello_log);
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[1]["id"], lines[0]["id"]);
    // An empty name re-renders the form, which is the 503 view for now.
    assert_eq!(
        handler
            .handle(post("/a/hello/name", "display_name=+"))
            .await
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );

    // animals: plant the first animal, start a round, walk one step, and forget.
    let planted = handler
        .handle(post("/a/animals/seed", "animal=wombat"))
        .await;
    assert_eq!(planted.status(), StatusCode::SEE_OTHER);
    assert_eq!(header(&planted, &LOCATION), "/a/animals/");
    let started = handler.handle(post("/a/animals/start", "")).await;
    assert_eq!(started.status(), StatusCode::SEE_OTHER);
    let lines = log_lines(&animals_log);
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["tbl"], "node");
    assert_eq!(lines[0]["d"]["kind"], "a");
    assert_eq!(lines[1]["tbl"], "cursor");
    assert_eq!(lines[1]["id"], "cursor");
    // Teaching is three events in one batch plus the cursor's tombstone (lib/tree.lua via
    // require, and pv.batch).
    let taught = handler
        .handle(post(
            "/a/animals/teach",
            "animal=penguin&question=Does+it+fly&answer=no",
        ))
        .await;
    assert_eq!(taught.status(), StatusCode::SEE_OTHER);
    let lines = log_lines(&animals_log);
    assert_eq!(lines.len(), 6);
    assert_eq!(lines[4]["d"]["kind"], "q");
    assert_eq!(
        lines[4]["id"], lines[0]["id"],
        "the leaf became the question"
    );
    assert_eq!(lines[5]["op"], "del");
    let reset = handler.handle(post("/a/animals/reset", "")).await;
    assert_eq!(reset.status(), StatusCode::SEE_OTHER);
    assert_eq!(log_lines(&animals_log).len(), 10);
    // An htmx caller gets the fragment, which is a view, so a 503 for now.
    let mut htmx = post("/a/animals/start", "");
    htmx.headers_mut()
        .insert("hx-request", "true".parse().unwrap());
    assert_eq!(
        handler.handle(htmx).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
}
