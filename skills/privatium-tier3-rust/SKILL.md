---
name: privatium-tier3-rust
description: Write Tier 3 Privatium apps in Rust by linking the privatium-core crate into your own binary. Covers the crate API, embedded mode, and when Tier 3 is warranted versus Tier 1 or 2. Load for hardware access, scheduled jobs, non-HTTP protocols, or native GUI and game clients.
---

# Privatium Tier 3 — Rust

Your binary, your `main()`, your routing. `privatium-core` supplies the log, materializer,
sync, discovery, and pairing as a library.

## When Tier 3 is right

Only when the HTTP surface genuinely cannot express what you need:

- A serial port, GPIO, USB device, or other hardware
- Scheduled or background work independent of a request
- A non-HTTP protocol (MQTT, Modbus, a custom TCP service)
- A native GUI or game that should not run in a browser

**If the app is records and forms, use Tier 1.** If it is a canvas or a web game, use
Tier 2. Tier 3 costs you the sandbox, hot reload, and the app-folder distribution model.

## Skeleton

```rust
use privatium::{Node, Event};

fn main() -> anyhow::Result<()> {
    let node = Node::open(&privatium::default_data_dir()?)?;
    node.serve_discovery()?;   // mDNS, UDP broadcast, PAKE pairing
    node.start_sync()?;        // iroh + LAN peers

    node.append("myapp", Event::put("reading", &privatium::ulid(),
        serde_json::json!({ "celsius": "21.4", "at": privatium::now() })))?;

    let rows = node.query("myapp",
        "SELECT * FROM reading ORDER BY at DESC LIMIT 100", &[])?;

    let app = axum::Router::new()
        .route("/", axum::routing::get(index))
        .layer(node.auth_layer());
    axum::serve(listener, app).await?;
    Ok(())
}
```

## API surface (stable across `api = 1`)

| Area | Calls |
|---|---|
| Lifecycle | `Node::open`, `close`, `default_data_dir` |
| Write | `append`, `append_batch` |
| Read | `query`, `query_one`, `get_row`, `events_since` |
| React | `subscribe` — fires for events arriving via sync too |
| Network | `serve_discovery`, `pair`, `start_sync`, `sync_now` |
| Auth | `auth_layer` (Tower middleware enforcing session and grants; `core::handle` applies it itself, so wrap your own router with it only in embedded mode) |
| Data | `snapshot`, `restore`, `restore_tier` |

## MUST

- Let the crate stamp `seq`, `lam`, `ts`, `dev`, and `app` — never set them
- Use `privatium::Decimal` for money; `f64` is a bug
- Resolve the data directory via `default_data_dir()` or the platform file-chooser portal.
  **Never** write beside the binary, and never require `--filesystem=host`
- Bind ports ≥ 1024; ACME is DNS-01 only
- Handle `subscribe` events from other devices, not just your own writes

## MUST NOT

- Open the SQLite file directly. It is a cache in `cache/`, rebuilt at will, and the
  materializer owns it.
- Write to another device's log file
- Mutate or truncate any log file
- Store secrets anywhere under `data/` — use the OS keyring
- Assume single-threaded access to a Lua state if you also embed Tier 1

## Anti-patterns

```rust
// WRONG: mutation
conn.execute("UPDATE reading SET celsius = ?1 WHERE id = ?2", ...)?;
// RIGHT
node.append("myapp", Event::put("reading", &id, json!({"celsius": "22.0"})))?;

// WRONG: float money
let total: f64 = rows.iter().map(|r| r.amount.parse::<f64>().unwrap()).sum();
// RIGHT
let total: Decimal = rows.iter().map(|r| r.amount.parse::<Decimal>()).sum::<Result<_,_>>()?;

// WRONG: breaks under Flatpak
let dir = std::env::current_exe()?.parent().unwrap().join("data");
// RIGHT
let dir = privatium::default_data_dir()?;
```

## Embedded vs host mode

A Tier 3 app is normally its **own binary** (embedded mode), not a folder in `apps/`. It
can register inside a host-mode node via the plugin trait, but that requires a custom build
of the node — which is exactly why Tier 2 exists.

## Verify

```bash
cargo test                      # the app's own suite; the linter reads app folders, not binaries
privatium new <slug> --tier rust    # the index entry a host-mode node lists (spec/app-contract.md §8)
privatium lint apps/<slug>          # that folder: manifest, slug, api, permissions (PV1xx, PV5xx)
privatium skill export privatium-tier3-rust   # this contract, matching the running version
```

Spec: `spec/app-contract.md §6`, `spec/protocol.md`.
