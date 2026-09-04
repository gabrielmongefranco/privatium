---
name: privatium-tier3-rust
description: Write Tier 3 Privatium apps in Rust by linking the privatium-core crate into your own binary. Covers the crate API, embedded mode, and when Tier 3 is warranted versus Tier 1 or 2. Load for hardware access, scheduled jobs, non-HTTP protocols, or native GUI and game clients.
---

# Privatium Tier 3 — Rust

Your binary, your `main()`, your routing. `privatium-core` supplies the log, the
materializer and the auth layer as a library now; discovery, pairing and sync are on the
same `Node` and arrive with Phases 2 and 3 of `docs/roadmap.md`.

## When Tier 3 is right

Only when the HTTP surface genuinely cannot express what you need:

- A serial port, GPIO, USB device, or other hardware
- Scheduled or background work independent of a request
- A non-HTTP protocol (MQTT, Modbus, a custom TCP service)
- A native GUI or game that should not run in a browser

**If the app is records and forms, use Tier 1.** If it is a canvas or a web game, use
Tier 2. Tier 3 costs you the sandbox, hot reload, and the app-folder distribution model.

## Skeleton

The repository's `crates/privatium-core/examples/embedded.rs` is this, whole, in thirty
lines, and CI runs it. The crate is `privatium-core`, used as `privatium_core`.

```rust
use std::net::SocketAddr;

use axum::{Router, routing::get};
use privatium_core::{Event, Node, new_ulid};
use serde_json::json;

const SCHEMA: &str = "CREATE TABLE reading (id VARCHAR PRIMARY KEY, celsius DECIMAL(5,1));";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut node = Node::open_with(None, None)?; // the platform data directory (spec/protocol.md §3)
    node.open_app("myapp", SCHEMA)?; // your app: no folder, its schema.sql inline

    node.append("myapp", Event::put("reading", new_ulid(), json!({ "celsius": "21.4" })))?;
    let rows = node.query("myapp", "SELECT celsius FROM reading WHERE celsius > ?", &[json!("20")])?;
    println!("{}", serde_json::to_string(&rows)?); // [{"celsius":"21.4"}] — DECIMAL stays a string

    let router = Router::new().route("/", get(index)).layer(node.auth_layer());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:8421").await?;
    let service = router.into_make_service_with_connect_info::<SocketAddr>();
    axum::serve(listener, service).await?;
    node.close()?;
    Ok(())
}
```

`Node::open(&dir)` opens a directory you chose; `Node::open_with(None, None)` the
platform's, which is what the `privatium` binary uses. Both take the root's lock: one
process per data directory (`spec/protocol.md §3.1`).

## API surface (stable across `api = 1`)

`spec/app-contract.md §6`, as the crate has it. `reference/api.md` lists every public
method of `Node` at this version, generated from the source.

| Area | Calls |
|---|---|
| Lifecycle | `Node::open`, `Node::open_with`, `open_app`, `close` |
| Write | `append` (one `Event`), `append_batch` (all or nothing) |
| Read | `query(app, sql, params)` — sandboxed, `?` bound, rows as `serde_json::Map` typed as `spec/data-api.md §1` types them |
| React | `subscribe(app)` — a `broadcast::Receiver<StreamEvent>` of every append and every resync; will carry events arriving by sync too |
| Network | `serve_discovery`, `pair`, `start_sync`, `sync_now` — present, and in a Phase 1 build every one returns `Error::Unimplemented` naming the phase; never `Ok` |
| Auth | `auth_layer` — Tower middleware; `core::handle` applies it itself, so wrap your own router with it only in embedded mode, and give the router `into_make_service_with_connect_info::<SocketAddr>()` so the layer sees the peer |
| Data | `snapshot`, `restore`, `restore_tier`, `maintain` |
| Ids | `new_ulid()` — the row key, minted by whoever writes the row |

## MUST

- Let the crate stamp `seq`, `lam`, `ts`, `dev`, and `app` — never set them; `Event` has
  no such fields
- Keep money as the strings `query` hands you — a declared `DECIMAL` or `BIGINT` arrives
  as a JSON string — and do arithmetic in SQL with `decimal_add`, `decimal_sum` and the
  rest of `spec/data-dictionary.md §2.1`, or with an exact decimal type of your own.
  `f64` is a bug.
- Resolve the data directory with `Node::open_with(None, None)` or the platform
  file-chooser portal. **Never** write beside the binary, and never require
  `--filesystem=host`
- Call `open_app` at every start, before the first `append` or `query`; an app never
  opened is `Error::AppNotLoaded`, not a silent success
- Bind ports ≥ 1024; ACME is DNS-01 only
- Match on `Error::Unimplemented` from the network calls and say so to your user, rather
  than assuming a sync happened
- Handle `subscribe` events from other devices, not just your own writes, once Phase 3
  delivers them

## MUST NOT

- Open the SQLite file directly. It is a cache in `cache/`, rebuilt at will, and the
  materializer owns it; `query` is the read path, and it refuses every write
- Write to another device's log file
- Mutate or truncate any log file
- Store secrets anywhere under `data/` — use the OS keyring
- Format a value into SQL — `query` binds `params`, and a `?` count that does not match is
  refused
- Assume single-threaded access to a Lua state if you also embed Tier 1

## Anti-patterns

```rust
// WRONG: mutation
conn.execute("UPDATE reading SET celsius = ?1 WHERE id = ?2", ...)?;
// RIGHT: an amendment is an append with the same id (spec/protocol.md §4.5)
node.append("myapp", Event::put("reading", &id, json!({"celsius": "22.0"})))?;

// WRONG: float money
let total: f64 = rows.iter().map(|r| r["celsius"].as_str().unwrap().parse::<f64>().unwrap()).sum();
// RIGHT: exact, in the engine
let rows = node.query("myapp", "SELECT decimal_sum(celsius) AS total FROM reading", &[])?;

// WRONG: a value formatted into the statement
let rows = node.query("myapp", &format!("SELECT * FROM reading WHERE id = '{id}'"), &[])?;
// RIGHT
let rows = node.query("myapp", "SELECT * FROM reading WHERE id = ?", &[json!(id)])?;

// WRONG: breaks under Flatpak
let dir = std::env::current_exe()?.parent().unwrap().join("data");
// RIGHT
let node = Node::open_with(None, None)?;

// WRONG: a router the layer cannot see the peer of — every caller reads as this process
axum::serve(listener, router.layer(node.auth_layer())).await?;
// RIGHT
let service = router.layer(node.auth_layer()).into_make_service_with_connect_info::<SocketAddr>();
axum::serve(listener, service).await?;
```

## Embedded vs host mode

A Tier 3 app is normally its **own binary** (embedded mode), not a folder in `apps/`. It
can register inside a host-mode node via the plugin trait, but that requires a custom build
of the node — which is exactly why Tier 2 exists. A folder with `tier = "rust"` under
`apps/` is an index entry a host-mode node lists (`spec/app-contract.md §8`) and nothing
more; the program is yours.

## Verify

```bash
cargo test                      # the app's own suite; the linter reads app folders, not binaries
privatium new <slug> --tier rust    # the index entry a host-mode node lists (spec/app-contract.md §8)
privatium lint apps/<slug>          # that folder: manifest, slug, api, permissions (PV1xx, PV5xx)
privatium skill export privatium-tier3-rust   # this contract, matching the running version
```

Spec: `spec/app-contract.md §2.3`, `§6`, `§7`; `spec/protocol.md`.
