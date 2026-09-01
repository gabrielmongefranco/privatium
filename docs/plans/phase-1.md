<!--
Project:  Privatium™
File:     docs/plans/phase-1.md
Authors:  Gabriel Mongefranco (@gabrielmongefranco)
Created:  2026-08-31
Modified: 2026-08-31
Summary:  Implementation plan for Phase 1 — a node that works on one machine.
          Non-normative. Where this plan and spec/ disagree, spec/ wins and this
          file is wrong.
-->

# Phase 1 Implementation Plan

Target: `docs/roadmap.md` Phase 1 — *a node that works on one machine*.

## 0. How to use this

Read `AGENTS.md` in full first, then `spec/protocol.md`, `spec/app-contract.md`,
`spec/data-dictionary.md`, and `docs/decisions/0003-in-process-adapter.md` (marked
**Phase 1** and load-bearing for M6). This plan is a work breakdown, not a substitute for
the contract.

One milestone per branch, one PR per milestone, in order. A milestone is done when its
checklist passes and its named tests are green — not when the code compiles. Do not start
M(n+1) before M(n) merges; several milestones deliberately re-open earlier files once the
next layer exposes what the earlier one got wrong.

Section 2 lists decisions this plan makes that the spec does not. **Confirm them before
M1.** Section 3 lists defects found in the spec; each must be fixed by a spec edit in the
PR that first depends on it, per `AGENTS.md`.

**A rule for the whole plan: do not invent CLI flags, `sys_*` column values, routes, or
config keys.** Every one of those surfaces is specified. If Phase 1 seems to need a new one,
that is a signal the spec is wrong and needs an edit in §3 — not a signal to add it quietly.
An earlier draft of this plan invented a `--bind` flag and a `kind = "console"` device, both
of which contradict the checked-in spec.

---

## 1. Scope

### In

`privatium-core` (log, store, app loader, Lua host, LSP compiler, the `Request`/`Response`
interface), the axum adapter, the HTMX shell, DuckDB materialization, snapshots and
three-tier restore, the Tier 2 data API and `pv.js`, and the CLI: bare `privatium`, `dev`,
`new`, `lint`, `snapshot`, `restore`, `skill`.

### Out — do not implement, do not stub, do not leave TODOs referencing

Pairing, session cryptography, cluster identity, device registry beyond this node's own row
(§2.2), mDNS, UDP broadcast, pkarr, DNS discovery, subtype advertisement, peer transport,
sync, relay, onion, Tauri shells, `uniffi`, packaging, `privatium pair`,
`privatium firewall`.

`sys_device`, `sys_app`, `sys_app_grant`, and `sys_audit` **are** in scope as tables — the
framework dogfoods its own log from day one — but grant resolution in Phase 1 always lands
on the `*+*` default and nothing writes a non-default grant.

`identity/cluster.*` and `identity/node.cert` are absent in Phase 1. That is a valid state,
and `sys_cluster` has zero rows.

### The one-sentence test

If a Phase 1 change would be observable to a second device, it is out of scope.

---

## 2. Decisions this plan makes — confirm before M1

The spec assumes a finished system. Phase 1 has no pairing, so six things must be decided
now rather than discovered in M6.

### 2.1 Phase 1 binds loopback only, and adds no flag to say so

`spec/protocol.md §8.2` forbids skipping the session layer on plain HTTP, and §13 lists
that as a conformance item. Phase 1 has no pairing and therefore no session keys, so the
only honest resolution is that Phase 1 never listens on a routable address.

- Default bind: `127.0.0.1`, port from `--port` (default 8420, `spec/cli.md §2`).
- **No `--bind` flag.** `spec/cli.md §2` defines `--port`, `--solo`, `--no-discovery`, and
  `--open`, and §10 is an explicit list of what the CLI deliberately lacks. Adding surface
  here would need a spec edit, and Phase 1 does not need one: binding loopback is a
  property of the phase, not a user choice.
- Phase 2 changes the bind address when pairing exists. It does not add a flag then either.

Startup prints the loopback URL and one line stating that LAN access arrives in Phase 2
(`spec/protocol.md §7`), so the limitation is visible rather than mysterious.

A side benefit worth naming: `127.0.0.1` **is** a potentially-trustworthy origin, unlike a
LAN IP (`docs/architecture.md §2.5`, ADR 0003). Phase 1 therefore runs in a secure context
for free, and any browser-side capability that needs one works during development and stops
working the moment Phase 2 binds the LAN. Do not build on that; know it.

`privatium --version` prints `pv/1 (partial: phase 1)` and does not claim conformance.

### 2.2 The node is the device — there is no "console device"

Events need a `dev`, `csrf()` needs a session, and `sys_device` needs a row. All three are
already satisfied by the node's own identity; no second identity is required.

- `spec/data-dictionary.md §3.2` gives `kind` a **normative** enum:
  `browser | desktop | mobile | node`. There is no `console`, and inventing one breaks the
  table for every later reader.
- `spec/data-dictionary.md §3.1` already states that every node appears in `sys_device`
  with `kind = 'node'`, including itself. Phase 1 writes exactly that row, with
  `replica = true` (`spec/protocol.md §1`, *Replica*: nodes always).
- `dev` on every event is the node ID from `identity/node.key`. No keypair is generated in
  `local/`.
- The HTTP layer treats any loopback request as this node. `auth_layer` exists with its
  real signature (`spec/app-contract.md §6`) and its Phase 1 body is
  "loopback → this node's device row, everything else → 403".

CSRF tokens are HMAC over `(node_id, session_nonce, path)` with a key held in `local/`, not
in `data/`, and not in the OS keyring (keyring access is Phase 2; do not pull it in).

Columns of `sys_device` that describe pairing — `paired_at`, `paired_via`, `ed25519_pub`,
`x25519_pub`, `user_agent` — are populated where trivially available and left NULL
otherwise. Do not fabricate a `paired_via` value; `lan | iroh | onion | tunnel` are all
wrong for a node that paired with nobody.

### 2.3 The cache may be mutated; the log may not — a clarification

`AGENTS.md` invariant 3 governs **log files**. Lint rule `PV303` governs **app SQL**.
Neither constrains the framework's privileged connection, which maintains
`cache/<slug>.duckdb` and may apply a new event with `DELETE` + `INSERT` rather than
re-replaying the whole log on every append.

This is not a new decision, but it reads like a violation at the call site. State it in a
comment there so the next reader does not "fix" it.

### 2.4 Route identity across pooled VMs

Lua handlers are values bound to one `mlua::Lua` state, and the pool holds N states
(`spec/lua-api.md §5`). So:

- Every VM in the pool loads the same `app.lua` and registers routes in the same order.
- Registration order defines a **stable route index**. The router holds
  `(method, pattern, index)` extracted from VM 0. A request matches a pattern, checks out
  any VM, and invokes handler `index` from that VM's registry table.
- If any VM produces a route table that differs from VM 0's by method, pattern, or count,
  the app fails to load with `app.load_failed` naming the divergence. Non-deterministic
  route registration is an app bug and must be loud.

Getting this wrong — by, say, keeping one "registration VM" and calling its functions from
request threads — is the single most likely way M7 produces something that works in
development and deadlocks under concurrency.

### 2.5 Named views live in `schema.sql`

See §3.2. `views.sql` is dead; `GET /a/<slug>/api/q/<view>` resolves against `CREATE VIEW`
statements in `schema.sql`, which `PV107` already permits.

### 2.6 Solo mode: framework prefixes win, and the linter says so

`spec/protocol.md §9.1` reserves `/`, `/settings`, `/api/v1/*`, `/skills/*`, and `/static/*`
for the framework. In **host mode** apps live under `/a/<slug>/` and cannot collide. In
**solo mode** the app owns `/`, and the spec does not say what happens when an app registers
`pv.get('/settings')`.

Phase 1 decides: **framework prefixes take precedence in both modes**, and an app route that
would be shadowed in solo mode is a load-time warning naming the route and the prefix. It is
a warning rather than a refusal because the same app is perfectly legal in host mode.

`spec/protocol.md §1.1` reserves the slugs `_sys`, `api`, `a`, `ws`, `static`, `health`,
`pair`, `well-known` — but not `settings` or `skills`, which are framework prefixes. See
§3.6.

---

## 3. Spec defects found — fix by editing `spec/`, not by working around

Each is a real contradiction in the checked-in specification, verified against `main`. Fix
the spec in the PR of the milestone that first depends on it.

### 3.1 `pv.action()` survives its own removal — `spec/data-api.md`

§2 ("Nothing else") states the `POST /a/<slug>/api/x/<action>` endpoint was removed with
the declarative tier. But §5 still lists `const result = await pv.action('learn', …)` in
the `pv.js` surface, and §4 still says `/api/schema` returns "available views and
**actions**".

**Fix:** delete the `pv.action` line from §5; change §4 to "available views". Do not
implement an action endpoint. *(Fix in M9.)*

### 3.2 `views.sql` survives its own removal — `spec/app-contract.md`, `spec/data-api.md`

`app-contract.md §1` says the declarative tier — "`views.sql` plus `forms.toml`" — was
removed. `app-contract.md §5` then lists `views.sql  OPTIONAL` in the Tier 2 layout, and
`data-api.md §1` says `/api/q/<view>` runs "a named view from `views.sql`".

**Fix:** strike `views.sql` from the §5 layout; change `data-api.md §1` to "a view defined
in `schema.sql`". `PV107` already allows `CREATE VIEW` there, so the mechanism exists and
only the pointer is wrong. *(Fix in M5.)*

### 3.3 `pv.url()` is referenced but never defined

`protocol.md §9.1`, `protocol.md §10`-era prose, and lint rule `PV301` all name `pv.url()`.
`lua-api.md §4` defines only the sandbox global `url()`.

**Fix:** define `pv.url` in `lua-api.md §3` as an alias of the global, and implement both.
Cheap, and it stops `PV301`'s own message citing a function that does not exist.
*(Fix in M7.)*

### 3.4 Lint fixtures cannot live in `apps/`

`docs/roadmap.md` requires "every lint rule … has both a passing and a failing case in
`apps/`". An app folder in `apps/` is loaded by a node; a folder that exists to fail
`PV101` or `PV203` must never be loaded.

**Fix:** put them in `apps/_lint/pass/<rule>/` and `apps/_lint/fail/<rule>/`. `_lint` is
not a valid slug (leading underscore fails `^[a-z][a-z0-9-]{1,30}$`), so the loader
already refuses it — but add an explicit skip for `apps/_*` in the loader so the refusal
is intentional rather than incidental, and update the roadmap wording to name the path.
*(Fix in M12.)*

### 3.5 Forward compatibility vs. projection — clarification, not a defect

`§4.2` requires unknown fields be preserved byte-for-byte. Materialization projects only
the columns in `schema.sql`. These do not conflict: preservation is a property of the log
file, which is never rewritten. Add one sentence to `§4.5` saying so, because an
implementer will otherwise try to round-trip unknown keys through DuckDB. *(Fix in M3.)*

### 3.6 `settings` and `skills` are framework prefixes but not reserved slugs

`protocol.md §9.1` reserves `/settings` and `/skills/*` for the framework. `§1.1` reserves
the slugs `_sys`, `api`, `a`, `ws`, `static`, `health`, `pair`, `well-known` — omitting both.
In host mode this is harmless. In solo mode an app named `settings` mounted at `/` is
ambiguous, and an app *route* `/settings` is shadowed silently.

**Fix:** add `settings` and `skills` to the §1.1 reserved list, and add one sentence to §9.1
stating that framework prefixes take precedence in both modes (§2.6). *(Fix in M6.)*

### 3.7 The `echo` acceptance test writes a `seq` gap

`§4.1` says `seq` **MUST be gapless**, and `§13` requires that sync reject gaps (§10.2).
`apps/hello/README.md` demonstrates hand-appending a line with `"seq":99` to a log whose
last `seq` is 3 — and that demonstration is a Phase 1 acceptance bullet in
`docs/roadmap.md`. Following it produces a log this node's own future sync must reject.

**Fix:** change the README example to the next sequential `seq`, and add one sentence to
§4.1: the **writer** MUST produce gapless `seq`; a **reader** MUST NOT reject, reorder, or
repair a gap it finds in a local file, because gap rejection belongs to sync (§10.2) where
the missing range can actually be requested. *(Fix in M10, with the reader behaviour
implemented in M2.)*

### 3.8 Fixed in this pass — no milestone needed

Four defects were typo-class and have been corrected directly:

- `spec/data-api.md §6` and `skills/privatium-tier2-web/SKILL.md` threw `BwOffline`, a
  leftover from the project's former name. Now `PvOffline`.
- `spec/protocol.md §9.1` said "reserves three prefixes" above a table listing five.
- `spec/data-dictionary.md §3.2` had the `sys_device` table split in half by an explanatory
  paragraph, so the second half rendered as body text rather than table rows.
- `AGENTS.md` invariant 1 had an unclosed quotation mark.

---

## 4. Workspace layout

```
Cargo.toml                    workspace
crates/
├── privatium-core/           the library; everything of consequence
│   ├── src/
│   │   ├── lib.rs            Node, Event, public API (app-contract §6)
│   │   ├── config.rs         config.toml, XDG paths, --data-dir
│   │   ├── identity.rs       Ed25519 node key, node ID, sys_device self-row
│   │   ├── log/              append-only writer, reader, Lamport, rotation
│   │   ├── store/            DuckDB materialize, snapshot, restore
│   │   ├── app/              app.toml, loader, lifecycle, permissions, CSP
│   │   ├── lua/              mlua host, sandbox, pool, pv module, lsp/
│   │   ├── wire/             Request/Response, core::handle, router (ADR 0003)
│   │   ├── http/             shell, settings, data API, SSE, auth_layer
│   │   ├── lint/             rules, fixer, json output
│   │   └── sys.rs            sys_* event helpers
│   ├── assets/icons/         vendored twbs/icons + LICENSE + VERSION
│   ├── examples/embedded.rs  the 30-line Tier 3 proof
│   └── tests/                spec-named integration tests
├── privatium/                the binary; clap, subcommands, axum adapter, terminal output
└── xtask/                    header check, icon verify, skill reference gen
apps/hello  apps/animals  apps/sketch  apps/_lint/{pass,fail}/
docs/plans/phase-1.md         this file
```

One core crate, per ADR 0002. `privatium-lint` does **not** become its own crate; the
linter is a module behind no feature flag, because CI and the binary must run identical
rules.

---

## 5. Dependencies

| Need | Crate | Note |
|---|---|---|
| HTTP | `axum`, `tower`, `tower-http` | `auth_layer` is a `tower::Layer` |
| Async | `tokio` | multi-thread runtime |
| Lua | `mlua` | features `lua54`, `vendored`, `send` |
| SQL | `duckdb` | feature `bundled`; see risk R1 |
| Crypto | `ed25519-dalek`, `sha2`, `hmac` | no session crypto in Phase 1 |
| IDs | `ulid` | Crockford Base32, 26 chars |
| Serde | `serde`, `serde_json` | `preserve_order` **off** — see below |
| Config | `toml`, `figment` or hand-rolled | manifest + config.toml |
| Watch | `notify` + debounce | `privatium dev` |
| Lua AST | `full_moon` | linter rules PV201/203/301/302/307 |
| CLI | `clap` (derive), `owo-colors` | **no `qrcode`** — QR is pairing, which is Phase 2 |
| Errors | `thiserror` in core, `anyhow` in the binary | `AGENTS.md` |
| Embed | `include_dir` | icons, shell assets, `pv.js` |

**Do not** parse event lines through `serde_json::Value` and re-serialize on any path that
writes or forwards. Sync is Phase 3, but the habit starts here: raw lines are `String`/
`&[u8]` end to end, and parsing is for reading only. This is `§4.2` and it is a conformance
item.

---

## 6. Milestones

### M0 — Workspace and guardrails

Scaffolding only. No behaviour.

- Workspace, three crates, MSRV pinned, `rust-toolchain.toml`.
- `xtask header-check`: every source file carries the standard header block (`AGENTS.md`,
  Style). Fails CI.
- `xtask icons-verify`: every icon name referenced anywhere in `apps/` and the shell
  exists in `assets/icons/`, per `docs/icons.md` and `PV503`.
- CI matrix: Linux, macOS, Windows. `fmt`, `clippy -D warnings`, `test`.
- Lint config denying `clippy::unwrap_used` and `clippy::expect_used` in
  `privatium-core`, allowed in `tests/` and in `main()` startup only.
- `deny.toml`: fail on GPL-2.0-only and non-commercial licences (ADR 0001 is a licence
  decision; make it mechanical). ADR 0004 §5 explains why this matters more than it looks.

**Done when:** CI is green on an empty workspace and `xtask header-check` fails when a
header is deleted.

---

### M1 — Identity, paths, configuration

- XDG data root resolution, `--data-dir`, `--config`, platform equivalents. Never write
  beside the binary (`AGENTS.md` 7).
- First run: create the directory tree of `protocol.md §3` exactly, generate the Ed25519
  node keypair, `0600` on `identity/node.key`, derive the node ID (first 40 bits of
  `SHA-256(pub)`, 8 chars lowercase Crockford Base32).
- Write this node's own `sys_device` row per §2.2: `kind = 'node'`, `replica = true`.
- `config.toml` parse with defaults; `[node] mode`, `port`, `lua.*` limits.
- Cluster key: **not** generated in Phase 1. `identity/cluster.*` and `identity/node.cert`
  absent is a valid state, and `sys_cluster` stays empty.

**Tests:** `test_spec_2_1_node_id_derivation` (fixed keypair → fixed ID),
`test_spec_3_layout_created`, `test_spec_2_1_key_mode_0600` (Unix only),
`test_identity_second_run_is_stable`, `test_sys_device_self_row_kind_is_node`.

---

### M2 — The event log

The heart. Get this wrong and nothing above it can be right.

- Writer: one file handle per `(app, own-node-id)`, `O_APPEND`, `\n` terminated (`0x0A`,
  never `\r\n`), `fsync` policy configurable and defaulting to sync-on-append (correctness
  over throughput; revisit with numbers, not vibes).
- `seq` per `(device, app)`, **gapless on write**, recovered on startup by reading the tail.
- **The reader tolerates gaps.** A gap in a local log file is not an error and must not be
  repaired, reordered, or rejected — see §3.7. Gap rejection is a sync concern (`§10.2`) and
  arrives in Phase 3.
- Lamport per app: `lam = max(lam_local, lam_max_seen) + 1`, persisted in `local/` and
  re-derived from the logs if `local/` is missing.
- Reader: `log/<dev>*.jsonl` treated as one stream ordered by `seq` (§3.2).
- **The live tail is plain, uncompressed JSONL and stays that way.** `AGENTS.md` invariant 1
  permits sealed historical segments to become compressed or Parquet later; `pv/1` seals
  nothing, so Phase 1 writes plain text only. Do not add a compression path, and do not
  design the reader so that adding one later means rewriting it: the reader takes a list of
  segments and asks each for a line iterator.
- Clock hygiene (§4.4): reject ingest > 24h ahead → `sys_audit`; warn on backwards jump
  > 60s.
- Batch append: contiguous `seq`, all-or-nothing. A partially written batch on crash must
  be detectable — write the batch in a single `write()` where the OS allows it, and on
  startup truncate nothing; instead report a trailing partial line as a load error naming
  the byte offset.

**Tests:** `test_spec_4_1_envelope_shape`, `test_spec_4_1_seq_gapless_on_write`,
`test_spec_4_1_reader_tolerates_gap`, `test_spec_4_3_lamport_monotonic`,
`test_spec_4_3_lamport_survives_restart`, `test_spec_4_4_future_ts_rejected`,
`test_spec_3_1_never_writes_other_device_log`, `test_spec_4_2_unknown_fields_preserved`,
`test_batch_is_atomic_under_kill`.

---

### M3 — Materialization

- Privileged DuckDB connection on `cache/<slug>.duckdb`; app-facing connection configured
  per `app-contract.md §7` with `lock_configuration = true` **last**.
- Parse `schema.sql` with DuckDB's own parser via `json_serialize_sql()` rather than a
  regex or a third-party SQL crate — it gives table names, column names, and types from
  the engine that will execute them, and it is what `PV106`/`PV107` should also use.
- Materialize each table as an explicit projection. No type inference anywhere:

```sql
CREATE OR REPLACE TABLE profile AS
WITH ev AS (
  SELECT seq, lam, ts, dev, op, id, d
  FROM read_json('<root>/data/hello/log/*.jsonl',
                 format = 'newline_delimited',
                 columns = {seq:'BIGINT', lam:'BIGINT', ts:'VARCHAR', dev:'VARCHAR',
                            app:'VARCHAR', op:'VARCHAR', tbl:'VARCHAR',
                            id:'VARCHAR', d:'JSON'})
  WHERE tbl = 'profile'
), ranked AS (
  SELECT *, row_number() OVER (PARTITION BY id ORDER BY lam DESC, ts DESC, dev DESC) AS rn
  FROM ev
)
SELECT id,
       CAST(json_extract_string(d, '$.display_name') AS VARCHAR) AS display_name
FROM ranked
WHERE rn = 1 AND op = 'put';
```

- Column list generated from `schema.sql`; `NOT NULL` and `CHECK` enforced before append
  (`data-api.md §2`), not after materialization.
- Schema-less apps: no tables, event log only (`sketch`).
- Incremental apply on append (§2.3), full rematerialize on `schema.sql` change, on
  restore, and on demand.
- `sys` schema for `_sys`, same machinery.

**Tests:** `test_spec_4_5_lww_by_lam_ts_dev`, `test_spec_4_5_row_granularity_not_field`,
`test_spec_4_6_tombstone_removes_row`, `test_spec_4_6_deleted_id_not_reusable`,
`test_spec_3_1_delete_cache_loses_nothing`, `test_decimal_arrives_as_string`,
`test_hand_appended_line_appears` (the `echo >>` case from `apps/hello/README.md`).

---

### M4 — Snapshots and three-tier restore

- `data/<slug>/snap/<ISO-year>-W<week>-<dev>-<hi_lam>/` with `MANIFEST.json`, `schema.sql`,
  `<table>.parquet`, `<table>.csv`. `MANIFEST.json` carries `parquet_sha256` and
  `csv_sha256` per table, exactly the shape in `§5.2` — do not add fields.
- Read precedence: Parquet + log tail → CSV + DDL + log tail → full replay. Record which
  tier was used and expose it on `pv.node()` and `/api/v1/...`.
- Tier 2 creates tables from `schema.sql` **before** loading CSV. No CSV type inference —
  `read_csv` with an explicit `columns` argument.
- Retention: 365 days default; never prune the last surviving snapshot; assert snapshot
  retention ≤ log retention (always true in `pv/1`, and the assertion is the point).
- Weekly scheduled snapshot; `privatium snapshot --verify`.

**Tests:** `test_spec_5_3_tier1_parquet`, `test_spec_5_3_tier2_on_parquet_corruption`,
`test_spec_5_3_tier3_on_csv_corruption`, `test_spec_5_4_never_prunes_oldest`,
`test_spec_5_1_snapshot_id_format`, `test_restore_reports_tier_used`.

---

### M5 — App loader

- Discover `apps/` under the data root **and** the repo's `apps/` in dev; skip `_*`
  (§3.4).
- `app.toml` parse and validate: required keys, slug regex, reserved slugs (§1.1 plus the
  two added in §3.6), directory-name match, `api` ≤ supported.
- Refusal is per-app and loud: `app.load_failed` event, node still starts
  (`app-contract.md §3.1`).
- Lifecycle per `app-contract.md §8`, **stopping before "advertise subtype"** — mDNS is
  Phase 2 and explicitly out of scope. Everything up to and including `mount` is Phase 1.
- Permissions → CSP string per app, computed at load. Default
  `script-src 'self'` scoped to the app path; `inline_script`, `wasm`, `eval`, `remote`
  each widen it and each is surfaced.
- `sys_app` upsert as an event.
- `sample/seed.jsonl` (`app-contract.md §9`): if present, offer to load it on first mount;
  never load it silently, and never load it into an app that already has events.
- **Do not relax the default CSP to make anything work**, and do not set `eval` or
  `inline_script` in a reference app to shorten a library's syntax (`AGENTS.md`). `animals`
  uses `@alpinejs/csp` precisely so this stays true. Relaxation is one-way: once `skills/`
  and the models reading them emit inline expressions, the permission can never be
  withdrawn.
- Fix spec defect §3.2 in this PR.

**Tests:** `test_spec_1_1_reserved_slug_refused`, `test_spec_3_1_slug_dir_mismatch_refused`,
`test_spec_12_higher_api_refused`, `test_broken_app_does_not_stop_node`,
`test_csp_default_blocks_inline_handlers`, `test_seed_not_loaded_over_existing_events`.

---

### M6 — `core::handle`, the axum adapter, and the shell

ADR 0003 is a **Phase 1** decision, and this milestone is where it is either honoured or
quietly lost. Build the interface first and the socket second.

- `core::handle(Request) -> Response` is the **only** entry point for application traffic.
  Every route in `protocol.md §9.1` — `/`, `/settings`, `/api/v1/*`, `/skills/*`,
  `/static/*`, `/a/<slug>/**` — is reachable through it with no socket involved.
- `Request` and `Response` bodies are **streams in both directions**, never `Vec<u8>`.
  Response streaming is what `/api/stream` needs in M9; request streaming is what a large
  upload needs. Retrofitting either is a rewrite, which is the entire reason ADR 0003
  exists before any adapter does.
- The axum layer is an **adapter**: socket, TLS-later, and body conversion. It adds no
  routes, rewrites no paths, and holds no routing table of its own. A platform that needs
  behaviour the others lack gets a capability flag in the core.
- `url()` / `pv.url()` is the only URL construction point. Host mode and solo mode differ
  inside it and nowhere else.
- Host mode launcher at `/`; **solo mode** mounts one app at `/` with no launcher and no
  `/a/<slug>` prefix. Both share one URL-building function so `url()` cannot drift.
- **Solo-mode precedence per §2.6:** framework prefixes win; a shadowed app route is a
  load-time warning. Fix spec defect §3.6 in this PR.
- Security headers of `§9.3` on every response; `Cache-Control: no-store` on anything
  carrying app data.
- `GET /api/v1/health` returns `{"v":1,"id":"..."}` only. `GET /api/v1/manifest` returns
  node ID, display name, app index, and the `pair` flag — no row counts, no activity
  timestamps, no app content.
- `/skills/<name>.md` and `/skills/bundle.zip` (`spec/cli.md §6`).
- `auth_layer` per §2.2. Loopback bind per §2.1.
- Shell: server-rendered HTML + HTMX, Bootstrap Icons inlined via `include_dir`. No
  bundler, no `node_modules` in the runtime path.
- Settings pages: node identity, installed apps, data directory, backup instructions. The
  devices page renders this node's own row and says pairing arrives in Phase 2.

**Tests:** all route tests are written as `handle(Request) -> Response` with no listener —
`test_spec_9_2_unauthenticated_leaks_nothing`, `test_spec_9_3_headers_present`,
`test_solo_mode_mounts_at_root`, `test_launcher_absent_in_solo_mode`,
`test_solo_mode_framework_prefix_wins`. Adapter-level:
`test_binds_loopback_only`, `test_adapter_registers_no_routes_of_its_own`,
`test_response_body_streams_without_buffering`,
`test_large_request_body_never_fully_buffered`.

---

### M7 — Lua host

The riskiest milestone. Budget accordingly.

- `mlua` 5.4 state construction with the sandbox of `lua-api.md §5`, which is a closed list:
  remove `io`; `os.execute`, `os.exit`, `os.getenv`, `os.remove`, `os.rename`, `os.tmpname`;
  `package.loadlib`, `package.cpath`; `debug`; `load`, `loadstring`, `dofile`, `loadfile`.
  Retain `os.time`, `os.date`, `os.clock`, `string`, `table`, `math`, `coroutine`, `utf8`.
- `require` replaced by a loader confined to the app's `lib/` plus a framework whitelist.
  Path traversal is the obvious attack; test it.
- All four limits, all required: instruction count via a debug hook installed **before**
  app code runs; memory via a custom allocator (`Lua::set_memory_limit`); wall clock checked
  in the same hook; pool size = CPU count. Exceeding any limit aborts the request, returns
  500, writes `lua.limit_exceeded`, and **must not** take down the node or poison the VM for
  the next request.
- VM pool and the stable route index of §2.4.
- The `pv` module: routing, `query`/`query1`/`get_row`, `append`/`delete`/`batch` with
  both arities, `ulid`, `now`, `device`, `node`, `setting`, `log`, `on('append', …)`,
  `dec`, `render`, `redirect`, `json`, `text`, `url`.
- Sandbox globals `url`, `icon`, `fmt.*`, `t`; template-only `render`, `layout`, `csrf`.
- `pv.dec` backed by an exact decimal type. `DECIMAL` and `BIGINT` cross the boundary as
  Lua strings, always.
- Fix spec defect §3.3 in this PR.

**Tests:** `test_spec_lua_5_banned_globals_absent` (one assertion per banned name, all
thirteen), `test_spec_lua_5_require_confined_to_lib`,
`test_spec_lua_5_instruction_limit_aborts`, `test_spec_lua_5_memory_limit_aborts`,
`test_spec_lua_5_wallclock_limit_aborts`, `test_spec_lua_5_limit_does_not_kill_node`,
`test_spec_lua_5_globals_are_per_vm`, `test_route_index_divergence_fails_load`,
`test_spec_lua_3_append_arities`, `test_spec_lua_3_batch_all_or_nothing`.

---

### M8 — LSP templates and hot reload

- Compiler for `<? ?>`, `<?= ?>` (always escaping, no flag), `<?raw ?>`, `<?-- --?>`.
- Emit a Lua chunk plus a line map from `.lsp` line → generated line, so a traceback
  points at the template the author wrote.
- Cache keyed by `(path, mtime, len)`; the compiled **source** is shared, the loaded chunk
  is per-VM. Generation counter invalidates both. `app-contract.md §8` compiles `views/` at
  load; the cache is what makes a later edit cheap, not a replacement for that.
- `layout`, `render` partials, `icon` inlining with `focusable="false"` and label
  handling.
- Error page: Lua traceback, offending template line with context, also written to the
  terminal (`spec/cli.md §3`).
- `privatium dev` watcher wiring, exactly the table in `spec/cli.md §3`: `views/*.lsp` →
  chunk cache; `app.lua`/`lib/*.lua` → app reload in place with routes re-registered;
  `static/*` → nothing; `schema.sql` → rematerialize; `app.toml` → manifest and routes
  re-read, data untouched. **No restart, ever.**

**Tests:** `test_lsp_escapes_by_default` (the `<script>alert(1)</script>` name),
`test_lsp_raw_emits_unescaped`, `test_lsp_error_maps_to_source_line`,
`test_hot_reload_template_next_request`, `test_hot_reload_app_lua_reregisters_routes`,
`test_hot_reload_schema_rematerializes`.

**Roadmap item satisfied:** "Editing a `.lsp` file is visible on the next request."

---

### M9 — Data API, `pv.js`, SSE

- `GET /a/<slug>/api/q/<view>` — named view from `schema.sql` (§2.5), `$name` binding,
  `limit`/`offset` with default 1000 and maximum 10000.
- `POST /a/<slug>/api/sql` — gated on `permissions.sql`, sandboxed connection, bound
  params only, reject `?` count mismatch rather than substituting, 20 req/s default.
- `GET /a/<slug>/api/row/<tbl>/<id>`, `GET …/api/events` (NDJSON, byte-identical lines).
- `POST /a/<slug>/api/events` — client supplies `op`/`tbl`/`id`/`d` only; **reject** a
  request that sets `seq`, `lam`, `ts`, `dev`, or `app` (`PV304`). Max 1000 events, 4 MB.
  Constraint validation before append; violation rejects the whole batch naming the
  offending index.
- `GET /a/<slug>/api/stream` — SSE, `append`/`resync`/`ping`, `after=` with no gap on
  reconnect. Served through the streaming `Response` body from M6, never buffered.
  `data-api.md §3` allows a long-poll fallback; Phase 1 does **not** implement it (ADR 0003
  defers the custom-scheme spike to the mobile shells). Because the body is stream-shaped,
  adding it later is a transport swap, not a refactor — which is the whole return on M6.
- `GET …/api/schema`, `GET …/api/node`.
- `pv.js` at `/static/pv.js`: ~4 KB, no dependencies, no build step. `query`, `sql`, `get`,
  `append`, `put`, `del`, `subscribe`, `ulid`, `node`, `online`, `on`. Outbox queue keyed
  by ULID with **no** dedupe table and no acknowledgement protocol (`AGENTS.md` 11,
  `PV305`). Offline reads throw `PvOffline`.
- `DECIMAL` stays a string in `pv.js`. No convenience conversion. That is the bug this
  design exists to prevent.
- Fix spec defect §3.1 in this PR — delete `pv.action` and "actions" from `spec/data-api.md`.

**Tests:** `test_spec_data_2_client_cannot_set_seq`, `test_spec_data_2_batch_atomic`,
`test_spec_data_2_batch_limits`, `test_spec_data_1_sql_requires_permission`,
`test_spec_data_1_param_count_mismatch_rejected`,
`test_spec_data_3_stream_no_gap_on_reconnect`, `test_spec_data_5_decimal_stays_string`,
`test_sketch_works_without_schema_sql`.

**Roadmap item satisfied:** "`sketch` (Tier 2) works with its own JavaScript and no
`schema.sql`."

---

### M10 — The three reference apps run

Not new subsystems — the integration milestone that proves M2–M9. Expect to reopen earlier
files; that is the purpose.

- `hello`: three routes, one table, two templates, no JavaScript. Fix spec defect §3.7 here
  — the README's `echo` example must not write a `seq` gap.
- `animals`: recursive SQL, `pv.batch` multi-event writes, stored cursor state, the
  HTMX/`is_htmx` branch **and** the no-JavaScript redirect branch, Alpine CSP build under
  the default CSP, the `_board.lsp` partial swap.
- `sketch`: canvas, `pv.js`, event log as document store, no SQL, no `schema.sql`.
- Accessibility baseline on the shell: labels, heading order, focus, contrast — the `PV4xx`
  rules apply to the framework's own HTML, not only to apps.

**Tests:** end-to-end per app: load, render, write, reload, verify against the log. Plus
`test_animals_works_with_javascript_disabled` and
`test_hello_readme_echo_example_is_valid` (parse the README's own command, run it, assert
gapless).

---

### M11 — CLI

Implement `spec/cli.md` and nothing beyond it.

- Bare `privatium [--port] [--solo <slug>] [--no-discovery] [--open]`. `--solo` overrides
  `[node] mode` for the run and is required by the Phase 1 solo-mode acceptance bullet.
  `--no-discovery` parses and is a no-op with a one-line notice, because there is no
  discovery to disable until Phase 2. `--open` opens a browser; it does **not** print a
  pairing QR code, because pairing does not exist yet.
- `privatium dev [--app <slug>] [--open]`.
- `privatium new <slug> [--tier lua|web|rust] [--from <app>] [--scaffold <table>]` — writes
  ordinary source files; **no runtime presence**, no config format that describes a UI.
- `privatium snapshot [--app] [--verify]`, `privatium restore --from [--app] [--dry-run]`.
- `privatium skill list|export [--out <dir>]`.
- Exit codes: `0`, `1` runtime, `2` usage, `3` lint findings.
- `privatium pair` and `privatium firewall` are specified but Phase 2 and Phase 6; they
  parse and exit with a clear "not in this build" message rather than being absent, so the
  help text matches the spec.
- No `doctor`, no `serve`, no `migrate`, no `install`, no `login` (`spec/cli.md §10`).

**Tests:** `test_cli_exit_codes`, `test_new_from_hello_rewrites_slug_and_title`,
`test_scaffold_output_passes_lint`, `test_no_undocumented_flags` (compare `clap` output
against the flags named in `spec/cli.md`).

---

### M12 — The linter

Ships in Phase 1 because it is what makes `skills/` enforceable rather than advisory.

- Implement every rule in `spec/cli.md §5.1`: `PV101–107`, `PV201–208`, `PV301–307`,
  `PV401–407`, `PV501–505`.
- Lua rules over a `full_moon` AST, not regex. SQL rules over `json_serialize_sql()`.
  HTML/template rules over the LSP parse tree from M8 — the linter reuses the compiler's
  front end rather than growing a second one.
- `PV502` (`cross_origin_isolated` only in solo mode) needs the solo-mode knowledge from
  M6; wire it, do not stub it. `docs/frameworks.md §5.4` explains why, and the
  `duckdb-wasm` note there is the case that will actually trip someone.
- `--format json`: every finding carries `id`, `severity`, `file`, `line`, `message`,
  `fix`, and a **resolvable** `spec` reference. Add an `xtask lint-spec-refs` that opens
  every referenced section and fails if one does not exist.
- `--fix` applies only mechanical corrections: literal mount path → `url()`, missing
  `focusable="false"`. Never SQL, never Lua control flow.
- Fixture corpus at `apps/_lint/{pass,fail}/<rule>/` (§3.4), one of each per rule, and a
  meta-test that **fails if a rule has no fixture pair**.

**Tests:** `test_lint_rule_<id>_passes` and `test_lint_rule_<id>_fails` generated over the
corpus; `test_every_rule_has_fixtures`; `test_reference_apps_lint_clean`;
`test_every_finding_has_resolvable_spec_ref`.

**Roadmap items satisfied:** the three lint bullets.

---

### M13 — Embedded mode, packaging-lite, release

- `examples/embedded.rs`: 30 lines, `Node::open` → `append` → `query` → own axum router
  with `auth_layer`. Compiled in CI, run in CI.
- Public API of `app-contract.md §6` present with real signatures. Phase-2 methods
  (`serve_discovery`, `pair`, `start_sync`, `sync_now`) either absent or returning a typed
  `Unimplemented` error — **not** silently succeeding. A no-op `start_sync` that returns
  `Ok` is a lie an embedder will build on.
- Single binary per platform, Linux/macOS/Windows, from CI. No installers — that is
  Phase 6.
- `xtask gen-skill-reference`: generate `skills/*/reference/*.md` from the crate and the
  spec; CI fails on drift, per `docs/skills.md §7`. Every spec edit made in M3, M5, M6, M7,
  M9, M10, and M12 must be reflected there or this fails, which is the intent.
- Fresh-clone check: `cargo build && ./privatium` on a clean machine produces a working
  `hello` at `http://127.0.0.1:8420`.

**Roadmap items satisfied:** the standalone-core bullet and the cross-platform bullet.

---

## 7. Conformance mapping

Phase 1 can satisfy exactly these lines of `protocol.md §13`, quoted as written there, and
CI should assert them by name:

| Checklist item (§13 wording) | Milestone |
|---|---|
| Deleting `cache/` and all `snap/` directories loses no data (§3.1, §5) | M3 |
| Preserves unknown envelope and `d` fields byte-for-byte (§4.2) | M2 |
| Lamport counter is monotonic across restart and sync (§4.3) — restart half only | M2 |
| Rejects events > 24h in the future (§4.4) | M2 |
| Row-granularity LWW ordered by `(lam, ts, dev)` (§4.5) | M3 |
| Three-tier read fallback, with the tier used recorded (§5.3) | M4 |
| Never prunes the oldest snapshot (§5.4) | M4 |
| Unauthenticated endpoints leak no app data (§9.2) | M6 |
| Refuses apps declaring a higher `api` (§12) | M5 |

Two `docs/roadmap.md` Phase 1 acceptance bullets are not in §13 and are easy to lose because
nothing else fails when they are: *every route reachable as `core::handle` with no socket*,
and *bodies stream in both directions*. Both are M6, and both need a test that would fail if
someone later "simplified" the adapter into a router.

Note what Phase 1 **cannot** claim. "Never writes to a log file for a device other than as
specified in §10.2" and "Sync rejects `seq` gaps (§10.2)" both reference sync, which does not
exist yet; M2's `test_spec_3_1_never_writes_other_device_log` is the Phase 1 subset, not the
conformance item. Everything else in §13 belongs to Phases 2, 3, and 5.

`privatium --version` prints `pv/1 (partial: phase 1)` and must not claim conformance until
they land.

---

## 8. Risks

**R1 — DuckDB `bundled` build.** Compiling DuckDB from source is a multi-minute C++ build
and inflates the binary substantially. It also makes "cross-compilation stays a one-liner"
(ADR 0002) optimistic — cross-compiling a bundled C++ dependency is not a one-liner. Test
this in M0 on all three CI platforms before M3 depends on it. If binary size is
unacceptable, the decision to revisit is *which extensions are statically linked*, never
*SQLite instead* — ADR 0001 §3 explains why the `DATE` requirement in particular is
load-bearing.

**R2 — `mlua` limits interacting.** The instruction-count hook and the memory limit are
separate mechanisms with separate failure modes, and an allocation failure inside a hook is
an unpleasant place to be. Write the limit tests first in M7 and make them adversarial: an
app that allocates in a tight loop, an app that recurses, an app that yields.

**R3 — Pooled VMs and hot reload.** The generation-counter design in M8 must handle a VM
checked out during a reload. Rebuild on checkout, not on the watcher thread.

**R4 — `full_moon` maintenance.** If it proves unmaintained or cannot parse Lua 5.4
constructs the reference apps use, the fallback is to lint via a sandboxed Lua parse in
`mlua` itself (parse without executing) plus token-level checks. Decide in M12, not M0.

**R5 — SSE plus hot reload.** A `resync` event on rematerialization is easy to forget and
produces a silently stale Tier 2 UI. M8 and M9 must be wired together, not sequentially.

**R6 — Streaming stops at Lua.** A Tier 1 handler returns a string; that is fine and
Tier 1 does not stream. But it means the streaming property of M6 must be a property of the
core boundary, not something each layer opts into — SSE and uploads must not be routed
through the Lua host at all. If you find yourself adding a chunked-return convention to
`pv.render`, stop: that is Tier 2's job via the data API.

**R7 — Scope drift toward Phase 2.** Every one of pairing, mDNS, and sync will look like a
small addition while you are in the HTTP layer. They are not. The loopback bind in §2.1 and
the lifecycle carve-out in M5 exist partly to make the boundary physical.

**R8 — Spec edits outrunning `skills/`.** Seven milestones edit `spec/`, and
`docs/skills.md §7` makes an unreflected spec change an incomplete change. The CI drift
check lands in M13, which is too late to be useful. Add it in M0 as a warning and promote it
to an error in M13.

---

## 9. PR sequence

| # | Branch | Depends on | Spec edits |
|---|---|---|---|
| 0 | `m0-workspace` | — | — |
| 1 | `m1-identity` | M0 | — |
| 2 | `m2-log` | M1 | §3.7 (reader half) |
| 3 | `m3-materialize` | M2 | §3.5 |
| 4 | `m4-snapshots` | M3 | — |
| 5 | `m5-app-loader` | M3 | §3.2 |
| 6 | `m6-wire-http-shell` | M5 | §3.6 |
| 7 | `m7-lua-host` | M6 | §3.3 |
| 8 | `m8-lsp-hot-reload` | M7 | — |
| 9 | `m9-data-api` | M6, M8 | §3.1 |
| 10 | `m10-reference-apps` | M9 | §3.7 (README half) |
| 11 | `m11-cli` | M10 | — |
| 12 | `m12-lint` | M11 | §3.4 + roadmap wording |
| 13 | `m13-embedded-release` | M12 | roadmap: tick Phase 1 |

---

Copyright © 2026 Gabriel Mongefranco
