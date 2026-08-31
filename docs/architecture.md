<!--
Project:  Privatium™
File:     docs/architecture.md
Authors:  Gabriel Mongefranco (@gabrielmongefranco)
Created:  2026-08-28
Modified: 2026-08-28
Summary:  Explanatory architecture overview. Non-normative; see spec/ for the contract.
-->

# Architecture

Non-normative. Explains *why* the system looks the way it does. For *what implementations
must do*, read `spec/protocol.md` and `spec/app-contract.md`.

## 1. The problem

Someone wants a small app for exactly one purpose — tracking medication fills, logging
migraines, cataloguing a workshop, whatever. They want it on their phone. They do not want
it on somebody's server. They cannot administer a server themselves, and if their laptop
dies they need a restore procedure they can explain to a relative over the phone.

Every existing answer fails at least one of those. Cloud SaaS fails privacy. Self-hosted
web apps fail administration. Local-only mobile apps fail multi-device. Anything with a
binary database fails the restore-over-the-phone test.

## 2. The six decisions everything else follows from

### 2.1 Plain text is the truth

All state is append-only JSONL. One file per device, never modified after writing. A
person with Notepad can read it. A person with any file sync tool can back it up. Two
devices can never conflict on a file because two devices never write the same file.

Everything else — the database, the snapshots, the CSVs — is a cache that can be deleted
and rebuilt.

This is the constraint that makes the restore drill possible: *copy the folder back*.

### 2.2 The query engine is DuckDB, and it is disposable

The framework replays JSONL into an in-process DuckDB instance, then runs the app's SQL
views against it. DuckDB earns the slot over SQLite for two specific reasons:

- **Real types.** `DECIMAL`, `DATE`, `INTERVAL`, `TIMESTAMPTZ` are native. An app tracking
  money and dates does not have to encode cents as integers and dates as strings.
- **It reads the truth directly.** `read_json_auto()` and `read_parquet()` query the log
  and snapshot files in place, so the "materializer" is a `CREATE TABLE AS SELECT`, not a
  subsystem.

The database file lives in `cache/` and is rebuilt on demand. Its format compatibility
across DuckDB versions is therefore irrelevant.

### 2.3 The framework is not an application model

The framework's job is storage, sync, discovery, pairing, encryption, and reachability.
It is deliberately **not** a prescription for how to build an application.

Three tiers, chosen per app, mixable on one node:

| Tier | Language | Fits |
|---|---|---|
| **1 — Lua** | Lua 5.4 + LSP templates | Records, lists, forms, reports, trackers |
| **2 — Web** | Your own HTML/JS/WASM against a data API | Games, canvas, charts, animation |
| **3 — Rust** | Rust against `privatium-core`, your own `main()` | Hardware, jobs, custom protocols |

Tiers differ by **language**, not by how much freedom you surrender. None has a ceiling.
Every tier gets the same storage, sync, discovery, pairing, encryption, snapshots, and
backup.

An earlier draft defined a declarative tier — an app was `schema.sql` plus `views.sql` plus
`forms.toml`. It was removed. It had a hard expressiveness ceiling, and it imposed an
application model the framework has no business imposing. What survives is a **scaffold
generator**: `privatium new --scaffold <table>` emits Lua and templates as ordinary source
files you then edit. A starting point you escape from, not a runtime you are trapped in.

Similarly there are three deployment modes: **host** (many apps at `/a/<slug>/`), **solo**
(one app at `/`, indistinguishable from a purpose-built application), and **embedded**
(your binary, the core as a library).

The failure mode to guard against is any tier quietly becoming mandatory because it is the
best-documented path. None of them is the product; the transport and the log are.

### 2.4 Instant feedback is a requirement, not a nicety

Tier 1 has no build step and no restart. LSP templates compile to cached Lua chunks
invalidated on file mtime; `app.lua` reloads in place. Save, refresh, done.

This is load-bearing rather than cosmetic. The people writing these apps are not
professional developers and are frequently working with an AI assistant. A loop measured in
seconds is the difference between iterating and giving up. If a change requires a restart,
that is a bug in the host.

### 2.5 The browser is a client, not the app

The framework's own UI — the shell, the launcher, settings — and every Tier 1 app render as
server HTML with HTMX. Application logic evaluates on the node, once, in one language, with
no second implementation to drift out of sync with the first.

The framework ships **no client-side framework**: it would need a build step, it would
duplicate state, and it would send hundreds of kilobytes to a phone to render one table.
HTMX is roughly 14KB.

This is a decision about the framework, not a rule imposed on apps. A Tier 2 app serves its
own `web/` directory and may use Canvas, WebGL, WASM, Three.js, Chart.js, or React if it
wants — at its own weight cost, which is the app author's call.

Icons are Bootstrap Icons, vendored as raw SVGs and inlined at render (`docs/icons.md`).
No icon font, no CDN, no runtime sprite fetch — which also means no additions to the
Content Security Policy and no broken glyphs offline.

This has a consequence worth stating plainly: **the browser client is online-only.**
Offline capability comes from the native shells (Tauri desktop, Tauri mobile) or from an
installed PWA on a stable HTTPS origin. That is a deliberate trade, not an oversight.

### 2.6 Certificates are a browser problem, not a security problem

Browsers demand CA-signed certificates because they do not know your node. Your own
software does. So:

- **Native clients** pin the node's public key at pairing and use X25519 + ChaCha20-Poly1305
  thereafter. No CA is involved. This is stronger than webPKI, not weaker.
- **Browser clients on LAN** run the same handshake in JavaScript over plain HTTP. This
  defeats passive eavesdroppers completely and detects active attackers after first
  pairing. It cannot protect a first contact that is already man-in-the-middled. That is
  the SSH trust model, stated honestly in `docs/security.md`.
- **Browser clients that need a real certificate** get one from a configured tunnel
  (Tailscale Serve) or a real domain with a DNS-01 issued certificate (DuckDNS). Both are
  optional.

## 3. Component map

```
┌───────────────────────────────────────────────────────────────────────────┐
│                         privatium-core  (Rust crate)                       │
│                                                                            │
│  log        append-only JSONL writer/reader, Lamport clock, replay         │
│  store      DuckDB materialization, snapshot write/read, three-tier restore│
│  app        app-folder loader, manifest validation, SQL sandbox            │
│  lua        mlua host, sandbox, VM pool, LSP compiler + hot reload         │
│  identity   Ed25519 node key, device registry, keyring access              │
│  pair       CPace/SPAKE2 handshake, code generation and rendering          │
│  session    X25519 + HKDF + ChaCha20-Poly1305 framing                      │
│  discover   DNS-SD, UDP broadcast, pkarr publish/resolve, DNS               │
│  peer       hole punching, relay fallback, direct QUIC transport           │
│  sync       pull/push protocol over any transport                          │
└───────────────────────────────────────────────────────────────────────────┘
        │                │                │                │
   ┌────▼────┐     ┌─────▼─────┐    ┌─────▼─────┐    ┌─────▼─────┐
   │ server  │     │  desktop  │    │  mobile   │    │  uniffi   │
   │ (daemon)│     │  (Tauri)  │    │  (Tauri)  │    │ (Swift/Kt)│
   └─────────┘     └───────────┘    └───────────┘    └───────────┘
```

The daemon and the Tauri shells are thin. Everything of consequence is in `privatium-core`,
which is why the mobile clients can be separate repositories that only depend on the crate
and the protocol.

## 4. Client tiers (how devices reach a node)

| Tier | Client | Reachability | Crypto | Offline | Third party |
|---|---|---|---|---|---|
| **0** | Any browser, LAN | `http://<ip>:8420`, mDNS | PAKE-derived session, TOFU-pinned | ✗ | none |
| **1** | Native desktop / mobile | LAN direct, then iroh | Pinned static keys | ✓ | none |
| **2** | PWA / browser, remote | Tailscale `ts.net` or DuckDNS + Let's Encrypt | TLS | ✓ (PWA) | one, opt-in |
| **2b** | Tor Browser | `.onion` (in-process Arti) | Tor | ✗ | none |
| **3** | Any file syncer | Syncthing / rsync / USB on `data/` | filesystem | ✓ | optional |

Tier 0 and Tier 1 satisfy the hard requirement: **a working path to the app and its data
with no third party and no certificate authority.** Tiers 2 and 3 are configuration.

## 5. Clusters and the shape of "peer"

Nodes belonging to one owner form a **cluster** sharing a keypair (`spec/protocol.md §2.3`).
A device pins the *cluster* key at pairing, not a node key, so pairing a phone once makes it
trust the desktop, the laptop, and any node admitted later.

That one change does most of the work people expect from peer-to-peer, without any of the
machinery. mDNS already returns several instances; clients filter on the cluster ID in the
TXT record and key on Node ID. Two nodes on a LAN discover and sync with no coordination, no
election, and no primary — because single-writer append-only logs make sync a set union.

Full per-client matrices for bootstrap and reachability are in `docs/connectivity.md`.

Beyond the LAN, a node publishes its addresses as **pkarr** records — DNS records signed by
its key, stored on the BitTorrent mainline DHT. The cluster's public key becomes its name,
with no registrar, no dynamic-DNS account, and no payment. Native clients then hole-punch
directly, falling back to a relay that forwards ciphertext it cannot read.

This is the account-free path, and it is why peer-to-peer is in `pv/1` rather than deferred.
It is a **native-client capability**: browsers cannot open raw sockets and cannot treat a
public key as a name, so remote browser access still requires a real domain and a
certificate.

An always-on machine remains useful for four separable jobs — relay, DNS, certificate host,
and full node — of which only the last holds data (`docs/deployment.md §2`).

**Clients are not all equally capable, and that is a runtime property, not a setting.** A
browser cannot browse mDNS, cannot open UDP, cannot pin a certificate, and cannot hold more
than one origin. Native shells can do all four. That gap — not rendering — is what justifies
building them. Full capability matrix in `spec/protocol.md §10.7`.

Mobile is a caching client with an outbox by default. A native mobile client MAY be a full
replica by embedding the core library; a browser never can.

## 6. Data flow

### Write
```
form submit / action invoke
  → params bound into the app's action SQL (sandboxed, read-only DB)
  → SELECT returns rows shaped (op, tbl, id, d)
  → framework stamps seq / lam / ts / dev / app
  → appended atomically to data/<app>/log/<this-device>.jsonl, fsync
  → in-memory DuckDB updated
  → HTMX fragment re-rendered
```

Forms are the degenerate case of an action returning one row. There is exactly one write
path in the system.

### Read
```
1. cache/<app>.duckdb fresh?              → query it
2. else: read_parquet(snap/**) + read_json_auto(log/**) WHERE lam > watermark
3. Parquet unreadable?  → CSV + schema.sql + log tail
4. Snapshots gone?      → full log replay from zero
```

Each fallback tier is logged loudly. A node that restored from tier 3 says so.

### Sync
```
peer A ──── "what's your highest lam per device?" ────▶ peer B
peer A ◀─── {dev1: 4192, dev2: 87, dev7: 12} ──────────  peer B
peer A ──── events for any (dev, lam) B is missing ────▶ peer B
```

Any peer, any direction, any number of them. A node that was powered off for a week is not a
special case — it is the ordinary catch-up path.

Because logs are append-only and single-writer, sync is a set union. There is no merge
algorithm, no vector clock reconciliation, and no conflict resolution step. Ordering for
last-write-wins is `(lam, ts, dev)` — see `spec/protocol.md §4`.

## 7. Multi-app hosting and solo mode

One node, many unrelated apps. The framework maintains an **app index** (`sys_app`, see
`spec/data-dictionary.md §3.4`) and mounts each app at `/a/<slug>/`.

Consequences of one node rather than one-node-per-app:

- One origin → one service worker, one credential, one pairing.
- One `data/` folder → one backup.
- One discovery record → the phone finds everything at once.
- Apps are isolated at the SQL level (separate DuckDB schemas) and at the log level
  (separate directories), but not at the process level. An app folder is trusted code in
  the sense that its SQL runs on your node — see `docs/security.md §6`.

Apps advertise themselves as DNS-SD subtypes, so a client that only cares about one app
can browse for `_meds._sub._privatium._tcp` and never see the rest.

**Solo mode** collapses all of this: one app mounted at `/`, no launcher, no slug prefix,
the app's name and icon become the node's. Same binary, same config file, one line
different. Use it when you are shipping *your app* rather than *a node that runs it*.

## 8. What is deliberately absent

| Not doing | Why |
|---|---|
| Multi-user accounts | Single-owner is the product. Sharing is a future protocol version, not a v1 feature bolted on. |
| A server, or a primary node | Every node is a peer. An always-on node is a peer that happens to be reachable. If a "server" role appears in an implementation, that is a defect. |
| A discovery registry | pkarr rides the mainline DHT. Nothing to bootstrap, nobody to operate it, nothing to register. |
| A dedupe table for the outbox | ULIDs make replay idempotent. Adding one signals a misreading of the merge rule. |
| CRDTs | Single writer per file makes them unnecessary. Reserved for concurrent free-text fields only, if ever. |
| A JavaScript build step | HTMX and three vendored crypto files. If it needs webpack, it is out of scope. |
| An admin UI | Configuration is a TOML file and a settings page. There is no second, hidden application. |
| Plugins / extensions | An app folder *is* the extension mechanism; Tier 3 is the compile-in path. |
| A declarative app format | Removed. See §2.3. The scaffold generator emits source, not config. |
| A charting library in the framework | Anything real is Tier 2, where you pick your own. See `docs/frameworks.md`. |
| An opinion about your front end | The framework serves your `web/` directory and gets out of the way. |
| Cloudflare Tunnel automation | Documented, not implemented. Requires a domain on their DNS; the value/maintenance ratio is poor. |

---

Copyright © 2026 Gabriel Mongefranco
