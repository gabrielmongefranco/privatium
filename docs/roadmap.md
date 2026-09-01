<!--
Project:  Privatium™
File:     docs/roadmap.md
Authors:  Gabriel Mongefranco (@gabrielmongefranco)
Created:  2026-08-28
Modified: 2026-08-31
Summary:  Build phases with explicit acceptance criteria. Non-normative.
-->

# Roadmap

Each phase ends with something usable. No phase depends on a later one to be worth
shipping. Acceptance criteria are written so that "done" is not a matter of opinion.

## Phase 1 — A node that works on one machine

**Deliverable:** a binary you run, that serves the `hello` app in a browser on the same
machine, and stores its data as JSONL.

Scope: `privatium-core` (log, store, app loader), **the `Request`/`Response` interface and
the axum adapter (ADR 0003)**, the Lua host (mlua, sandbox, VM pool),
the LSP compiler with hot reload, HTTP server, HTMX shell, DuckDB materialization,
snapshots, three-tier restore, the Tier 2 data API and `pv.js`, and the CLI including
`privatium dev`, `new`, and `lint` (`spec/cli.md`).

**Done when:**
- [ ] `hello` and `animals` (Tier 1) load, render, and accept writes
- [ ] `sketch` (Tier 2) works with its own JavaScript and no `schema.sql`
- [ ] Editing a `.lsp` file is visible on the next request — no restart, no build
- [ ] The Lua sandbox rejects `io`, `os.execute`, and `debug`, and enforces all four limits
- [ ] Solo mode serves one app at `/` with no launcher
- [ ] `privatium lint` passes on all three reference apps and fails on seeded violations
- [ ] Every lint rule in `spec/cli.md §5` has both a passing and a failing case under
      `apps/_lint/pass/<rule>/` and `apps/_lint/fail/<rule>/` — not in `apps/` proper, where
      the loader would try to mount them
- [ ] `--format json` findings each carry a resolvable `spec` reference
- [ ] `privatium dev` reloads Lua, templates, and schema with no restart
- [ ] `privatium-core` compiles and runs standalone in a 30-line embedded example
- [ ] Every application route is reachable as `core::handle(Request) -> Response` with no
      socket, and the HTTP server is a thin adapter over it (ADR 0003)
- [ ] `Request` and `Response` bodies are streams in both directions — `/api/stream` is
      served without buffering, and a large upload never lands in memory whole
- [ ] `rm -rf cache/ data/*/snap/` then restart → identical state
- [ ] A hand-written JSONL line appended by `echo` appears in the UI after reload
      *(this is the test that keeps `AGENTS.md` invariant 1 honest — the live tail stays
      plain, uncompressed JSONL no matter what sealed segments become)*
- [ ] Conformance checklist items for §3, §4, §5 pass
- [ ] Runs on Linux, Windows and macOS from a single binary

## Phase 2 — Other devices on the LAN

**Deliverable:** open the app on your phone by scanning a QR code.

Scope: pairing (CPace/SPAKE2), emoji pad + word codes, session crypto in Rust and JS,
device registry, mDNS + UDP discovery, key pinning.

**Done when:**
- [ ] Pairing completes on a phone in under 20 seconds, without a keyboard
- [ ] Word-code path completes with the screen reader on and images disabled
- [ ] Wireshark on the LAN shows no plaintext application data
- [ ] Changing the node key produces the full-screen refusal with no override
- [ ] Two nodes on one LAN are distinguishable in the discovery list by ID, not name
- [ ] Conformance checklist items for §6, §7, §8 pass

## Phase 3 — More than one node

**Deliverable:** desktop and laptop stay in sync with no server, and one pairing covers both.

Scope: cluster identity and node admission, node certificates, sync protocol over LAN HTTP,
filesystem watcher for externally-synced logs, endpoint candidate list with failover.

**Done when:**
- [ ] A second node is admitted with one pairing; the phone reaches it **without re-pairing**
- [ ] Discovery filters to your own cluster on a LAN carrying a stranger's node
- [ ] **The power-cut case:** desktop off, phone syncs to laptop, desktop wakes and catches up
      with no conflict and no lost writes
- [ ] Edit offline on both machines, reconnect, both converge
- [ ] Syncthing on `data/` alone produces the same convergence with sync disabled
- [ ] A `seq` gap is detected and repaired rather than appended
- [ ] Lamport counters survive restart and remain monotonic
- [ ] Cluster private key is absent from every event, snapshot, and backup export
- [ ] Killing the active endpoint fails over in under 5 seconds, not 30

**Sync demo, once §10 works:** wire `animals` to `/api/stream` with the HTMX SSE extension.
Teaching an animal on the desktop makes the phone's history update live, on screen, with no
polling code and no page reload. Two devices, one visible cause and effect — a far better
demonstration than a passing test, and it costs one attribute.

## Phase 3b — The always-on node

**Deliverable:** the phone works on cellular.

Scope: documentation and a VPS quickstart. **No new protocol** — an always-on node is an
ordinary cluster member.

**Done when:**
- [ ] A VPS node is admitted with the same flow as a laptop
- [ ] Phone on cellular, both home machines off, reads and writes still work
- [ ] Destroying and rebuilding the VPS node loses nothing
- [ ] Nothing in the codebase distinguishes it from any other node

## Phase 4 — Native shells

**Deliverable:** installable desktop app, and Android and iOS apps.

Scope: Tauri v2 desktop, Tauri mobile, `uniffi` bindings, **`privatium-ffi` (the C ABI)**,
offline read + write outbox. Mobile clients live in separate repositories depending on
`privatium-core`.

The shells are adapters over `core::handle` (ADR 0003), so they add no routing work. The
desktop shell gets offline for free: the core is in-process, so there is no service worker,
no PWA manifest, no certificate, and no domain — see `docs/architecture.md §2.5`.

**Open risk, carried deliberately:** custom-scheme *streaming* in a platform webview,
particularly WKWebView, is unproven. `spec/data-api.md §3` therefore specifies long-poll as
a conformant fallback for `/api/stream`. Because `Response` is stream-shaped in the core,
this is a transport swap rather than a refactor. The spike belongs to the mobile
repositories, not to Phase 1.

**New reference app: `lantern` (Tier 3).** A deliberately trivial LÖVE game — one button,
dodge falling shapes, run ends — linking `privatium-ffi` through LuaJIT's FFI. Each run
appends one event. Paired with a small Tier 1 app rendering run history, personal bests, and
per-device statistics in the browser: start a run on the desktop, see it on the phone. The
game is trivial on purpose; the demonstration is the C ABI and the log, not the gameplay.

**Done when:**
- [ ] Desktop app works with the network cable unplugged
- [ ] Mobile app pairs, syncs, and survives airplane mode with queued writes
- [ ] Native mDNS discovery works on Android (NSD) and iOS (Bonjour, with the local
      network permission prompt handled)
- [ ] **The Wi-Fi-to-cellular transition:** switch mid-session, app keeps working, queued
      writes replay, no re-pairing, no duplicated rows
- [ ] Native clients hold and fail over a multi-endpoint list; browser clients hold one
- [ ] `sys_device.replica` is reported accurately by every client kind, and **reachability
      is reported separately** — a phone is a full replica whose reachability is
      foreground-only (ADR 0005)
- [ ] Mobile resolves discovery records but does not publish them by default; publishing is
      a setting, off by default
- [ ] `lantern` runs as a native LÖVE binary against `privatium-ffi` with no node process,
      and its paired Tier 1 app renders the same runs in a browser

## Phase 5 — Reaching home from outside

**Deliverable:** the app works on cell data with **no account, no domain, and no payment**,
and installs as a PWA for those who want one.

### 5a — pkarr discovery

Small, useful alone, and does not constrain 5b. Replaces DDNS for anyone willing to forward
one port.

- [ ] Node publishes signed records to the mainline DHT under its own key
- [ ] Records stay under 1000 bytes and carry no application data
- [ ] Republishes on a timer and on address change; a sleeping node vanishes within hours
- [ ] Uses BEP44 mutable items, never BEP5 infohash announcements
- [ ] Publishing is disableable independently of resolving
- [ ] Concurrent DNS resolution works on a network with the DHT blocked

### 5b — Direct peer transport

- [ ] Phone on cellular reaches a home node with no VPN, tunnel, DDNS, or account
- [ ] Laptop elsewhere reaches the desktop at home, same conditions
- [ ] Relay fallback works when hole punching fails; `p2p.relay_only` forces it
- [ ] A self-hosted relay is configurable and the public default is disableable
- [ ] Audit distinguishes `p2p.direct` from `p2p.relayed` so an owner can see which is in use

### 5c — Routes retained

- [ ] Mesh VPN path works with zero Privatium configuration
- [ ] DuckDNS + Let's Encrypt issues and auto-renews without inbound ports
- [ ] PWA install prompt appears only on a secure context and never on plain HTTP
- [ ] `.onion` address resolves and serves, with the manual `torrc` route documented
- [ ] Cloudflare Tunnel documented, not implemented
- [ ] mDNS and the LAN address still work with every one of the above disabled

## Phase 6 — Packaging

**Deliverable:** install it the way your distribution expects.

Scope: `.deb`, `.rpm`, AppImage, Flatpak, MSI, notarized `.app`, and per-OS firewall
guidance (`docs/deployment.md §4`).

**Done when:**
- [ ] Fresh installs accept an inbound LAN connection on Windows, macOS, Debian, Ubuntu,
      Fedora, and openSUSE — or explain in plain language what to run, without demanding it
- [ ] The node never requires administrator privileges to run
- [ ] mDNS works, i.e. UDP 5353 is handled as its own rule and not forgotten
- [ ] Flatpak build passes with no `--filesystem=host`
- [ ] Owner-chosen data directory works through the file-chooser portal, sandboxed
- [ ] Autostart uses the Background portal
- [ ] mDNS works inside the sandbox

## Phase 7 — The first real app

**Deliverable:** the medication fill / prior-authorization tracker, in a separate
repository, as an app folder.

This is the proof. If it needs a framework change to work, the framework was wrong and the
change belongs in `pv/1` before the app ships.

## Ongoing, not a phase

`skills/` ships and versions with the code. A change to `spec/` without the matching skill
update is incomplete (`AGENTS.md`). `privatium lint` is what makes the skills enforceable
rather than advisory, so it lands in Phase 1, not later.

## Open questions, not yet scheduled

Each of these is worth prototyping before it is worth specifying. None is a deliverable.

### Tier 1 rendering offline

Tier 1 renders on the node, so a cached shell gives offline access to views already
*visited*. Rendering an unvisited view needs handler logic in the browser. Three options,
in increasing ambition:

1. **Accept the limit.** Offline Tier 1 = visited views plus queued writes. Probably
   sufficient — this morning's list, this evening's entry. Zero new machinery, and this is
   the specified behaviour until something replaces it.
2. **Ship Lua to the browser.** `wasmoon` runs Lua 5.4 in WASM. Run the *same* `app.lua` and
   the *same* compiled LSP templates client-side — no second implementation and therefore no
   drift, which is the objection that rules out client frameworks in the first place. The
   open problem is the query layer: Tier 1 handlers run SQL against DuckDB.
3. **Tier 2.** Already works. The author owns their client code.

### `duckdb-wasm` for offline Tier 1 queries

Fast in practice, and OPFS-backed persistence is real. Two things to settle before it is
adopted:

- **Payload over cellular.** Measure the gzipped size of the non-threaded (`eh`) bundle.
  That number decides it.
- **Single-threaded bundle only.** The multithreaded build requires cross-origin isolation,
  which would break host mode for every other app on the node (`docs/frameworks.md §5.4`).
  This constraint holds regardless of how the benchmarks come out.

If `duckdb-wasm` does not work out, option 2 above still stands with a narrower offline
query surface. It is not load-bearing for it.

### Passing data between node Lua and browser Lua

Useful if browser Lua happens, and worth keeping even if `duckdb-wasm` does not. **The
mechanism already exists: it is the event log.** Events are JSON, JSON maps to Lua tables,
and both sides already agree on the shape. Do not build a second serialisation path, a
shared-state abstraction, or transparent RPC — those work in a demo and leak at every
failure boundary. Keep it explicit and JSON-shaped.

### PWA client replica

Wanted for people on a real HTTPS origin, and clearly the second path after the native
shell — build the shell first, since it needs no replica at all. When built, it is the event
log's `(dev, lam)` watermark plus an outbox, roughly 300 lines, not a third-party sync
library (`docs/decisions/0004 §2`).

## Explicitly not on the roadmap

Multi-user sharing, an app registry, a plugin API, cloud hosting, a mobile SDK for third
parties, and a hosted sync relay. Each of these turns a personal tool into a service, which
is the thing this project exists to avoid.

And no `doctor` subcommand. Failures should be detected and explained where they occur.

---

Copyright © 2026 Gabriel Mongefranco
