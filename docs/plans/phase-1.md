<!--
Project:  Privatium™
File:     docs/plans/phase-1.md
Authors:  Gabriel Mongefranco (@gabrielmongefranco)
Created:  2026-08-31
Modified: 2026-09-05
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
M1.** Section 3 records the spec defects found while writing it — all already fixed in
`spec/`, so the checked-in specification and this plan agree.

**A rule for the whole plan: do not invent CLI flags, `sys_*` column values, routes, or
config keys.** Every one of those surfaces is specified. If Phase 1 seems to need a new one,
that is a signal the spec is wrong and needs an edit in §3 — not a signal to add it quietly.
An earlier draft of this plan invented a `--bind` flag and a `kind = "console"` device, both
of which contradict the checked-in spec.

---

## 1. Scope

### In

`privatium-core` (log, store, app loader, Lua host, LSP compiler, the `Request`/`Response`
interface), the axum adapter, the HTMX shell, SQLite materialization, snapshots and
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

The spec now says everything §3 found it should say. What remains here is the set of
decisions that are genuinely Phase-specific: things true only because pairing does not exist
yet, or true only of this implementation. Six of them.

### 2.1 Phase 1 binds loopback only, and adds no flag to say so

`spec/protocol.md §8.2` forbids skipping the session layer on plain HTTP, and §13 lists that
as a conformance item. Phase 1 has no pairing and therefore no session keys, so the only
honest resolution is that Phase 1 never listens on a routable address.

- Default bind: `127.0.0.1`, port from `--port` (default 8420, `spec/cli.md §2`).
- **No `--bind` flag.** `spec/cli.md §2` defines `--port`, `--solo`, `--no-discovery`, and
  `--open`, and §10 is an explicit list of what the CLI deliberately lacks. Adding surface
  here would need a spec edit, and Phase 1 does not need one: binding loopback is a property
  of the phase, not a user choice.
- Phase 2 changes the bind address when pairing exists. It does not add a flag then either.

**Three sentences of `spec/cli.md §2` describe behaviour Phase 1 cannot have**, and the
divergence should be visible rather than silently absent:

| §2 says | Phase 1 | Why |
|---|---|---|
| "begins discovery (`§6`)" | no discovery | mDNS and pkarr are Phase 2 and Phase 5 |
| "prints the LAN URL" | prints the loopback URL | there is no LAN listener |
| "`--open` additionally prints a QR code for pairing" | `--open` opens a browser | pairing is Phase 2 |

Startup also prints one line saying LAN access arrives with pairing (`spec/protocol.md §7`),
so a reader of the spec is not left wondering.

A side benefit worth naming: `127.0.0.1` **is** a potentially-trustworthy origin, unlike a
LAN IP (`docs/architecture.md §2.5`, ADR 0003). Phase 1 therefore runs in a secure context
for free, and any browser-side capability that needs one works during development and stops
working the moment Phase 2 binds the LAN. Do not build on that; know it.

`privatium --version` prints `pv/1 (partial: phase 1)`, which `spec/cli.md §1` now requires
of any build that does not satisfy every item in `§13`.

### 2.2 The node is the device — there is no "console device"

Events need a `dev`, `csrf()` needs a key, and `sys_device` needs a row. All three are
already satisfied by the node's own identity; no second identity is required.

- `spec/data-dictionary.md §3.2` gives `kind` a **normative** enum:
  `browser | desktop | mobile | node`. There is no `console`, and inventing one breaks the
  table for every later reader.
- `spec/data-dictionary.md §3.1` already states that every node appears in `sys_device` with
  `kind = 'node'`, including itself. Phase 1 writes exactly that row, with `replica = true`
  (`spec/protocol.md §1`, *Replica*: nodes always).
- `dev` on every event is the node ID derived from `identity/node.key`.
- The HTTP layer treats any loopback request as this node. `auth_layer` exists with its real
  signature (`spec/app-contract.md §6`) and its Phase 1 body is "loopback → this node's
  device row, everything else → 403".

**CSRF keying, and why no new file appears.** `AGENTS.md` invariant 5 says keys live in the
OS keyring or `identity/`, never in `data/` — and `spec/protocol.md §3` shows `local/`
holding `state.jsonl`, nothing else. Writing a CSRF secret to `local/` would violate the
first and quietly extend the second, which is the same mistake as the `--bind` flag.

So Phase 1 stores no CSRF secret at all. The key is derived at startup:

```
csrf_key = HKDF-SHA256(ikm = node private key, info = "privatium/csrf/v1")
```

and the token is `HMAC-SHA256(csrf_key, node_id ‖ session_nonce ‖ path)`, with the nonce
held in memory for the process lifetime. Restarting the node invalidates outstanding forms,
which is correct and unremarkable on a single machine. No keyring access in Phase 1.

Columns of `sys_device` that describe pairing — `paired_at`, `paired_via`, `ed25519_pub`,
`x25519_pub`, `user_agent` — are populated where trivially available and left NULL
otherwise. Do not fabricate a `paired_via` value; `lan | iroh | onion | tunnel` are all wrong
for a node that paired with nobody.

### 2.3 The cache may be mutated; the log may not — a clarification

`AGENTS.md` invariant 3 governs **log files**. Lint rule `PV303` governs **app SQL**. Neither
constrains the framework's own connection, which maintains `cache/<slug>.sqlite` and
may apply a new event with `DELETE` + `INSERT` rather than re-replaying the whole log on
every append.

This is not a new decision, but it reads like a violation at the call site. State it in a
comment there so the next reader does not "fix" it.

### 2.4 Route identity across pooled VMs

Lua handlers are values bound to one `mlua::Lua` state, and the pool holds N states
(`spec/lua-api.md §5`). So:

- Every VM in the pool loads the same `app.lua` and registers routes in the same order.
- Registration order defines a **stable route index**. The router holds
  `(method, pattern, index)` extracted from VM 0. A request matches a pattern, checks out any
  VM, and invokes handler `index` from that VM's registry table.
- If any VM produces a route table that differs from VM 0's by method, pattern, or count, the
  app fails to load with `app.load_failed` naming the divergence. Non-deterministic route
  registration is an app bug and must be loud.

Getting this wrong — by, say, keeping one "registration VM" and calling its functions from
request threads — is the single most likely way M7 produces something that works in
development and deadlocks under concurrency.

### 2.5 Incremental materialization is an optimization, not a second code path

`spec/protocol.md §4.5` defines materialization as a full replay. Doing that on every append
is O(log) per write and unusable by the time a log has 50,000 lines.

Phase 1 keeps both paths and makes them checkable against each other: the incremental apply
must produce byte-identical table contents to a full replay of the same log. `M3` carries a
property test that appends a random event stream, applies it incrementally, replays from
scratch, and diffs. If they ever differ, the incremental path is wrong — the replay is the
definition.

Full rematerialization is unconditional on `schema.sql` change, on restore, and on demand.

### 2.7 The engine is SQLite — decided after M6

`docs/decisions/0006`. Everything above and below that names DuckDB was written before
that decision and is kept as the record of what M3 to M6 built; the code, the spec and the
docs say SQLite. What it changes for the milestones still ahead:

- **M7:** `pv.dec` is `store::Decimal`, already written; `pv.query` runs on
  `Store::app_conn()`, a read-only sandboxed connection that needs no window.
- **M9:** `columns` in a query result carry `Column::ty`, the declared type; `sys.v_*` is
  `cache/_sys.sqlite` attached read-only as `sys` on the app's connection.
- **M12:** `PV107` is now answerable — the authorizer sees every statement's action while
  `schema.sql` runs — and two rules the engine made necessary join the list: `SUM()` over
  a `DECIMAL` column, and `+` on a `DATE` (`spec/data-dictionary.md §2`).

### 2.6 `_sys` is an app, and it bootstraps before any other

`sys_device`, `sys_app`, `sys_app_grant`, and `sys_audit` live in `data/_sys/`
(`spec/protocol.md §3`), which means the framework writes its own event log through the same
writer apps use. Nothing in the spec says what happens on the very first run, when the node
must write its own `sys_device` row before a log, a Lamport counter, or a materialized `_sys`
schema exists.

Order for M1, and it only works in this order:

1. Create the tree, generate the keypair, derive the node ID.
2. Open the `_sys` log writer with `lam = 0`, `seq = 0`.
3. Append the `sys_device` self-row and the `sys_node` row.
4. Materialize `_sys`.
5. Only then load `apps/`.

`_sys` is not discoverable, not mountable, and not lintable — the loader skips it exactly as
it skips the lint fixture corpus. It is a reserved slug (`§1.1`) precisely so this stays true.

---

## 3. Spec defects found — all fixed, none deferred

Every contradiction found while writing this plan has been corrected in `spec/` and `docs/`
already. An implementer should not be handed a specification they have been told is wrong,
so nothing here is left for a milestone PR. This section is the record of what changed and
why, not a to-do list.

| # | Was | Now | Files |
|---|---|---|---|
| 1 | `pv.action()` and "actions" survived the removal of the declarative tier that created them | Both struck; no action endpoint exists | `data-api.md §4, §5` |
| 2 | `views.sql` survived the same removal, and `/api/q/<view>` still pointed at it | Named views resolve against `CREATE VIEW` in `schema.sql`, which `PV107` already permitted | `app-contract.md §5`, `data-api.md §1` |
| 3 | `pv.url()` named by `PV301` and `protocol.md §9.1`, but only `url()` defined | `pv.url` specified as an alias; implementations MUST provide both | `lua-api.md §4.0` |
| 4 | Lint fixtures required to live in `apps/`, where the loader would mount them | Fixtures live under `apps/_lint/{pass,fail}/<rule>/` | `roadmap.md` Phase 1 |
| 5 | Unknown-field preservation appeared to conflict with column projection | One paragraph stating it does not, and forbidding round-tripping unknown keys through the query engine | `protocol.md §4.5` |
| 6 | `/settings` and `/skills/*` were framework prefixes but not reserved slugs, and solo-mode shadowing was undefined | Both slugs reserved; framework prefixes take precedence in both modes with a load-time warning | `protocol.md §1.1, §9.1` |
| 7 | The `echo` acceptance test wrote `"seq":99` into a log whose last `seq` was 3 — a permanent gap, against a MUST | Example uses the next `seq`; writer MUST be gapless, reader MUST NOT reject a gap | `protocol.md §4.1`, `apps/hello/README.md` |
| 8 | `--version` unspecified, though the conformance disclaimer depends on it | Specified, and a non-conformant build MUST qualify the protocol string | `cli.md §1` |
| 9 | Solo-mode shadowing was warn-at-load only, so CI never saw it | New lint rule `PV506` | `cli.md §5.1` |
| 11 | `protocol.md §4.1` said the envelope `id` is a ULID, unqualified, while `data-dictionary.md §3.1`/`§3.2` key `sys_node` and `sys_device` by Node ID and `lua-api.md §3.3` lets a caller supply its own — `apps/animals` ships `id = 'cursor'` | `§4.1` now says row key, ULID by default, with the two exceptions named and the cross-device collision consequence stated | `protocol.md §4.1` (found in M1) |
| 10 | `BwOffline`; "three prefixes" over a five-row table; `sys_device` table split by a paragraph; unclosed quotation mark | Corrected | `data-api.md`, `privatium-tier2-web/SKILL.md`, `protocol.md §9.1`, `data-dictionary.md §3.2`, `AGENTS.md` |
| 12 | `app-contract.md §7` described a privileged connection and a sandboxed one coexisting over one app's cache. DuckDB makes all four of those settings `GLOBAL_ONLY` and locks the database file exclusively, so that arrangement cannot be built — and an implementation that appeared to have both would have sandboxed neither | §7 now specifies the boundary as open-privileged → materialize → seal → serve, with rematerializing and snapshotting needing a fresh instance. *Superseded by ADR 0006: with SQLite the two connections do coexist, and §7 describes the read-only one* | `app-contract.md §7` (found in M3) |
| 13 | `§4.6`'s "an `id` that has been deleted MUST NOT be reused" forbade `apps/animals`, which deletes and recreates its `'cursor'` singleton every round on a key `§4.1` explicitly blesses | `§4.6` now names what it protects — a **minted** ULID must not become the key of a different row — and states that a caller-chosen key may be re-asserted, that enforcement is the data API's, and that materialization follows `§4.5` regardless | `protocol.md §4.6` (found in M3) |
| 14 | `§4.6` justified "a replay follows `§4.5` over whatever the log contains" by citing `§4.1` as forbidding a reader to reject what it finds. `§4.1` forbids rejecting a `seq` gap, specifically — and `§4.4` affirmatively **requires** rejecting a future-dated event, which the materializer does | The paragraph now says a replay follows `§4.5` over whatever survives `§4.4`'s clock hygiene, the only filter a reader is required to apply, and that `§4.1`'s mercy for `seq` gaps is the same principle. Behaviour unchanged; the citation was wrong | `protocol.md §4.6` (found in the M3 audit) |
| 15 | `§5.2`'s example manifest said `"engine": "duckdb 1.4.3"` while the bundled engine is 1.5.5, and nothing said what the field holds | `engine` is the engine's name plus its own reported version; the example is marked illustrative. *Now `sqlite <version>` (ADR 0006)* | `protocol.md §5.2` (found in M4) |
| 16 | `§5.3` named a "log tail" without defining it. An event with `lam ≤ hi_lam` that a snapshot never saw — `§4.1`'s cross-device case, or a hand-appended line — had no correct merge against snapshot rows that carry no `(lam, ts, dev)`, and neither a changed `schema.sql` nor a log behind the snapshot was addressed | The tail is the sane events with `lam > hi_lam`; a snapshot applies only when that set is exactly what its `hi_seq` did not cover, the log holds everything `hi_seq` claims, and its `schema.sql` matches; an inapplicable snapshot is tier 3 without being a failure; the tier is node-local state, with `restore.tier2`/`restore.tier3` audits per `data-dictionary.md §3.10` | `protocol.md §5.3` (found in M4) |
| 17 | `§5.1` left the width of `<week>` unspecified (`W5` or `W05`) | Two digits, zero-padded, as ISO 8601 spells it | `protocol.md §5.1` (found in M4) |
| 18 | `cli.md` PV502, `docs/frameworks.md §5.4` and three skills name `permissions.cross_origin_isolated`; `app-contract.md §5.4`'s `[permissions]` block did not define it, and nothing said what shape a `remote` entry takes before it is written into a header | Defined there, solo mode only and refused at load in host mode; `remote` entries MUST be origins | `app-contract.md §5.4` (found in M5) |
| 19 | `§8` placed "upsert sys_app" after validation, so a validation failure could never reach `sys_app.last_error`, which `§3.4` says holds "load/validation failure text" | The row is keyed by the folder name; every folder whose name is a valid, unreserved slug has a row whether or not it loaded, with `last_error` when it did not; a folder that cannot be keyed gets the audit alone; `installed_at` is the first clean load; the row is amended only when it would change | `app-contract.md §8`, `data-dictionary.md §3.4` (found in M5) |
| 20 | `§3.4` listed `source ∈ bundled \| local \| url:<origin>` and defined none of them; `§3.1` refused a colliding app without saying which of the two | `bundled` and `local` defined, `url:` reserved; local is discovered before bundled and the second folder for a slug is the one refused | `data-dictionary.md §3.4`, `app-contract.md §3.1` (found in M5) |
| 21 | `§9` named `sample/seed.jsonl` and said nothing about its shape, when it loads, or as whose events | One envelope per line; `op`, `tbl`, `id`, `d` taken and the envelope's `seq`/`lam`/`ts`/`dev`/`app` discarded; appended through the node's own log; never automatic; refused when the log already holds an event | `app-contract.md §9` (found in M5) |
| 22 | `§12` refused an app whose `api` exceeds what the node "implements", while `cli.md §1` has a Phase 1 build call itself partial — leaving what it implements undefined | `api` is the app contract's version and a positive integer; a `pv/1` build implements `api = 1` whether or not it satisfies `§13`; the `--version` qualifier is about conformance | `protocol.md §12` (found in M5) |
| 23 | `§9.2` listed the manifest's four fields and defined none of them: no JSON shape, no answer for a `display_name` that is NULL, and a `pair` flag for a pairing that does not exist yet | The object is spelled out; `name` falls back to the Node ID while unset; `pair` is whether a pairing is open now and is `false` in a build without pairing; `apps` is the mounted apps as slug, title and icon | `protocol.md §9.2` (found in M6) |
| 24 | `cli.md §6` served `/skills/bundle.zip` without saying what is in it, and `skills/README.md`'s `curl … | bsdtar -xf-` assumed an answer | Every file under `skills/` at its repository-relative path, stored not compressed, so extracting in place reproduces the tree | `cli.md §6` (found in M6) |
| 25 | `§9.3` listed one CSP with no scope, while `app-contract.md §5.4` gave each app its own; whether the shell's pages were bound by the listed one was unsaid | The listed policy is the framework's own, on every response it renders as written — the shell inlines nothing — and the floor each app's policy starts from, with `script-src` path-scoped per `§5.4` | `protocol.md §9.3` (found in M6) |
| 26 | `§9.3`'s "every response containing app data" named no set, so `no-store` on a stylesheet and `no-store` on nothing were both readings | Everything except the embedded `/static/*` assets and the `/skills/*` documents; the four headers are on every response, refusals included | `protocol.md §9.3` (found in M6) |
| 27 | `app-contract.md §6` called `auth_layer` a Tower middleware, and ADR 0003 forbids an adapter from adding anything, which left unsaid whether `core::handle` applies it or each adapter does | It is a `tower::Layer`; `core::handle` applies it itself, so adapters do nothing; an embedder wraps their own router with it (`§2.3`) | `app-contract.md §6` (found in M6) |
| 28 | `§9.1` described solo-mode shadowing in terms of `pv.get(…)` routes, which a Tier 2 app has none of | A Tier 2 app's routes are its `web/` paths, so a top-level entry named after a prefix is what is shadowed, and the warning names it that way | `protocol.md §9.1` (found in M6) |
| 29 | `lua-api.md §3.2` said `DECIMAL` and `BIGINT` arrive as strings but not how a result column finds its declared type — the cache declares storage types, and `sqlite3_column_decltype` returns `TEXT`/`INTEGER` — nor what an expression such as `count(*)` arrives as, nor what `BOOLEAN` and `JSON` become | A column originating in a declared column (directly or through a view, by column-origin metadata against the schema) is typed by its declaration: `DECIMAL`/`BIGINT` strings, `BOOLEAN` a boolean, `JSON` decoded; a computed column arrives by storage class, so `count(*)` is a number and `decimal_sum()` a string; NULL is an absent key | `lua-api.md §3.2` (found in M7) |
| 30 | `§3.2` promised `/` on `pv.dec` while `store::Decimal` has no division on purpose — a quotient is not exact — and its arithmetic saturates | No `/`: `a:div(b, scale)` at an explicit scale, rounded half away from zero; `pv.dec` errors on overflow and refuses a float; the SQL functions keep saturating | `lua-api.md §3.2`, `data-dictionary.md §2.1` (found in M7) |
| 31 | `docs/icons.md` spelled the helper `icon('trash', { label = … })` with a `size` option; `lua-api.md §4`, both skills and every app wrote `icon('trash', 'Delete this fill')` | The string form; no options table, no `size` — an icon is `1em` | `docs/icons.md`, `lua-api.md §4.1` (found in M7) |
| 32 | `§3.4`'s `pv.node()` named `name`, `peers` and `restore_tier` and defined none of them before pairing and sync exist | `name` falls back to the Node ID as `§9.2` does, `peers` is `0` until pairing, `restore_tier` is the tier of `§5.3` or `nil` | `lua-api.md §3.4` (found in M7) |
| 33 | `§3.4`'s `pv.setting('key', default)` said nothing about an unset key or the value's type | `default` when no row has the key (`nil` without one); the value JSON-decoded | `lua-api.md §3.4` (found in M7) |
| 34 | `§3.4` said `pv.log` must "never write to stdout directly" and named no destination, and `print` was unaddressed | The node's diagnostic log — standard error, prefixed by the slug — never the event log or `sys_audit`; `print` goes there as `info` | `lua-api.md §3.4` (found in M7) |
| 35 | `§3.1`'s `req.device` was "the paired device's ID" in a phase with no pairing | The device the request was authenticated as: this node's own ID in Phase 1 (`§2.2`) | `lua-api.md §3.1` (found in M7) |
| 36 | `§3.4` said `pv.on('append')` fires for synced events "too" without saying it fires for the node's own appends, when, or in which VM | Every event this node appends — a handler's writes and the owner's seed — synchronously after the write, in the VM that wrote, once per event; re-entrant appends fire it again | `lua-api.md §3.4` (found in M7) |
| 37 | `§3.2`'s `get_row` for a tombstoned id was unspecified against `protocol.md §4.6` and `Store::is_tombstoned` | `nil`, as for an absent id — `§4.5` materializes no row for a tombstone, and the data API answers 404 for both | `lua-api.md §3.2` (found in M7) |
| 38 | `data-dictionary.md §3.10`'s normative `kind` list omitted `lua.limit_exceeded`, which `lua-api.md §5` requires | Added, `warn`, subject the app, detail naming route, limit and measure | `data-dictionary.md §3.10` (found in M7) |
| 39 | `§5`'s closed list left `os.setlocale`, which is process-wide state | Removed as well | `lua-api.md §5` (found in M7) |
| 40 | `§5` said exceeding a limit "aborts the request", but the hook's error is an ordinary Lua error a `pcall` can catch, and a long SQL statement runs where the hook cannot fire | The verdict is the host's: 500 and the audit row whether or not the error was caught, the hook re-armed to every instruction so a `pcall` loop cannot continue, the VM discarded and rebuilt; SQL under the connection's progress handler with the same deadline; a memory error the app recovers from is not an exceedance | `lua-api.md §5` (found in M7) |
| 41 | `§4.1` required `csrf()` in every non-GET form and said nothing about who verifies the token, or how a request without a form (`hx-delete` on a button, `§4`'s own example) carries it | The host verifies a mount-scoped token on every non-GET request beneath the mount, from the `_csrf` field or an `X-CSRF-Token` header; enforcement lands with the templates that emit it | `lua-api.md §4.1` (found in M7) |
| 42 | `§4.1`'s `t('key')` was "if `locales/` exists" with no `locales/` format anywhere, and `fmt.*` named no rule | `t` returns the key unchanged in `pv/1`; `fmt.date`/`fmt.money`/`fmt.rel` defined against `ui.date_format` and `ui.locale`, unparsable values returned unchanged | `lua-api.md §4.1` (found in M7) |
| 43 | `§3.3` said nothing about how a Lua table becomes `d` — sequences, objects, empty tables, floats — or about `pv.append` inside `pv.batch` | The encoding rule; `pv.append`/`pv.delete` inside a batch and a nested batch are errors; one `ts` per batch | `lua-api.md §3.3` (found in M7) |
| 44 | Row 29 had `BIGINT` arrive as a Lua string "for the reason in `§2.1`", but that reason — JSON numbers are doubles — does not apply to Lua, whose integer is 64-bit; every SQLite binding hands an `INTEGER` back as an integer | The rule is SQLite's own: `INTEGER` → integer, `REAL` → float, `TEXT` → string, so `BIGINT` is a Lua integer and `DECIMAL` (text) a string; `BOOLEAN` → boolean and `JSON` → table stay as the two conveniences | `lua-api.md §3.2` (M7 review) |
| 45 | Row 30 dropped `/` from `pv.dec`; a method-only division was judged too awkward for authors and models | `/` exists, at the larger scale of the operands, rounding half away from zero; `:div(b, scale)` stays for a named scale | `lua-api.md §3.2`, `data-dictionary.md §2.1` (M7 review) |
| 46 | `§2.1` said a value that does not parse as its type "materializes as NULL" and left the write side to the data API, so `pv.append('fill', { filled_on = '3/9/2026' })` would have written a string SQLite's date functions cannot read | Typed writes: every value naming a declared column is normalized before the append — digits, booleans, and the accepted date, time and timestamp spellings to ISO, `ui.date_format` deciding `3/9` — and a value that is not its type refuses the append naming the column; the NULL rule is for lines that reached the log another way | `lua-api.md §3.3`, `data-dictionary.md §2.1` (M7 review) |
| 47 | `§5` said global state is "per-VM and not shared" and left a global assigned in a handler visible to every later request on that VM — PHP's cross-request state, with its leaks | A global assigned in a handler lasts one request; `app.lua`'s definitions are the baseline; mutating a baseline table in place is the remaining per-VM footgun the linter checks | `lua-api.md §5` (M7 review) |
| 48 | `§4.1` never said what a view that calls no `layout()` renders inside, or when a view's output is the whole response; neither reference app calls `layout()`, both depend on the shell's stylesheet, and `apps/animals/views/_assets.lsp` says the app does not own the document | A view with no `layout()` renders inside the framework's page frame — head, the app's title, shell.css, htmx, a header, `<main>`, the view's own `<h1>`; a request htmx makes (`HX-Request` without `HX-Boosted`) gets the output alone; `layout('base')` is how an app owns the document | `lua-api.md §4.1`, `app-contract.md §4.2` (found in M8) |
| 49 | `§4` said `<?= ?>` "always escapes" while its own example, both apps and `docs/icons.md` wrote `<?= icon(...) ?>`, `<?= csrf() ?>` and `<?= render(...) ?>` — an unconditional escaper renders those as visible `&lt;svg` | Those helpers return an HTML value that `<?= ?>` emits as it is; every other value is escaped; concatenating the value into a string loses the marker and is escaped again; `nil` emits nothing, a table or function is an error naming the line; a comment is stripped whatever it contains | `lua-api.md §4`, `app-contract.md §4.2`, `docs/icons.md`, `AGENTS.md` (found in M8) |
| 50 | `§4.1`'s `layout('base')` said "wrap" and nothing about what the layout sees or when it runs | It runs after the view with the same ctx plus `content`; the view `pv.render` named may call it, a partial may not | `lua-api.md §4.1` (found in M8) |
| 51 | `§2` listed `static/` in the Tier 1 layout and both apps link `url('/static/…')`, but nothing served it beneath a mount — every such link was a 404 | `<mount>static/*` is the framework serving the app's `static/`, as it serves a Tier 2 app's `web/`; a route never sees those paths | `lua-api.md §2` (found in M8) |
| 52 | `protocol.md §9.1` made `/static/*` the framework's outright in both modes, which in solo mode left a Tier 1 app's own stylesheet unreachable through `url()` | The framework's embedded names come first and, in solo mode, a name it lacks falls through to the mounted Tier 1 app's `static/`; a Tier 2 app's `web/static/` stays shadowed | `protocol.md §9.1` (found in M8) |
| 53 | `cli.md §3` read as if reloading were a `dev`-only mode — "runs a node with file watching enabled" — while `lua-api.md §4`, `§7` and `architecture.md §2.4` make no-restart reloading the host's own behaviour | The table in `§3` is what the host does on every run, noticed by a stat on the next request; `dev` is the front door and adds only its flags. A save that does not load is the error page until the next save loads, never the code from before it | `cli.md §3`, `lua-api.md §7` (found in M8) |
| 54 | `§4.1`'s header path for the token — `X-CSRF-Token` for `hx-delete` on a button — had no source a template could put the token into | The page frame sets `hx-headers` on `<body>`, so every htmx request beneath the mount carries it; a view owning its document uses `hx-headers` or `hx-include="[name=_csrf]"`; `_csrf` stays in `req.form` | `lua-api.md §4.1` (found in M8) |
| 55 | `§4` said nothing about `<?= nil ?>`, a number, a boolean, a table | `nil` → nothing; number, boolean, a value with `tostring` → its text, escaped; table or function → an error naming the line | `lua-api.md §4` (found in M8) |
| 56 | `§4.1` made ctx keys bare names and said nothing about a name absent from the ctx; `apps/hello` and `apps/animals` keyed their message `error`, which is Lua's `error` function, so `<? if error then ?>` was always true and `<?= error ?>` emitted a function | A name absent from the ctx is the sandbox global or `nil`; a key MUST NOT be a Lua builtin's name; the reference apps say `err` | `lua-api.md §4.1`, `apps/hello`, `apps/animals` (found in M8) |
| 57 | `data-api.md §1` bound query-string parameters to `$name` placeholders in a `CREATE VIEW`; SQLite refuses a parameter inside a view ("parameters are not allowed in views"), so no schema that used the feature could have loaded | The framework rewrites `$name` to `pv_param('name')` at load — a scalar function on every connection that the API fills from the query string and that is NULL anywhere else; a view's placeholders are listed by `/api/schema`, and a query-string key naming none is refused | `data-api.md §1, §4`, `lua-api.md §3.2`, `app-contract.md §7` (found in M9) |
| 58 | Every endpoint "requires a live session; cookies carry it; no token handling is required in app code", and nothing kept a page on another origin from riding the owner's cookie into an API that takes no token | A POST is read only as `application/json`, which no cross-origin page can send without a preflight the node never answers; a request a browser marks `Sec-Fetch-Site: cross-site` is refused on every route; no token, because `pv.js` has no page frame to read one from and a native client has no page | `data-api.md §2.1`, `AGENTS.md` (found in M9) |
| 59 | `§5` listed the helper's functions and none read the log, while `apps/sketch` booted through a `pv.events()` that existed nowhere; `pv.get` was destructured as `{ d }` while `/api/row` answers 404 | `pv.events(filter)` is an async iterator over `/api/events`; `pv.get` returns `null` for a 404; `pv.lam`, `pv.mount`, `pv.on('resync' \| 'rejected')` and what `pv.append` returns when queued are spelled out | `data-api.md §5, §6`, `app-contract.md §5.3`, `apps/sketch`, `privatium-games` (found in M9) |
| 60 | `/api/row` said "single row by ULID" and nothing about its shape — a materialized row, which a schema-less app has none of, or an event | The winning event's log line, `d` holding the row, for a declared and an undeclared table alike | `data-api.md §1` (found in M9) |
| 61 | `§3` showed `append` with a subset of the envelope, gave `resync` one reason and `ping` no body, and said nothing about what `after=` sends first or what a slow reader gets | `append` is the log line; `resync` is `rematerialized` or `lagged`, with the high-water mark; `ping` carries it too, and its stat of the app is what notices a log that grew on an idle node; `after=` sends the events past it from the log, then live ones, subscribed and read under one hold of the lock; `HEAD` is headers alone; 429 past `api.max_streams` | `data-api.md §3` (found in M9) |
| 62 | `§7` named five `api.*` settings that `data-dictionary.md §3.6`'s reserved list lacked, and no deadline bounded a statement on `/api/q` or `/api/sql` | The keys are in `§3.6`; a statement runs under `[lua] max_seconds`, the deadline Tier 1's SQL already had | `data-api.md §7`, `data-dictionary.md §3.6` (found in M9) |
| 63 | `§2` had `NOT NULL` and `CHECK` validated for the API alone; `lua-api.md §3.3`'s typed writes ran for every writer and `Node::append` is the one write path, so `pv.append` would have written a row the API refused | Constraints hold on every write path — `pv.append`, `pv.batch`, the seed, the API — by the author's own DDL run in a throwaway database, naming the event's index | `lua-api.md §3.3`, `data-api.md §2` (found in M9) |
| 64 | Nothing said what a refusal looks like | `{"error", "index"?, "column"?}` with the status, on every route | `data-api.md` preamble (found in M9) |
| 65 | `data-dictionary.md §4` had apps reading `sys.v_*` and `app-contract.md §7` had the authorizer refusing `ATTACH`, with nothing between them | The framework attaches `cache/_sys.sqlite` read-only as `sys` before the authorizer goes on; it cannot be detached; `pv.query` and `/api/sql` read it | `app-contract.md §7`, `data-dictionary.md §4`, `lua-api.md §3.2` (found in M9) |
| 66 | `protocol.md §9.1` made `/api/*` the framework's outright in both modes, which in solo mode left the solo app's data API unreachable | `/api/v1/*` is the framework's; the rest of `/api/` beneath the solo mount is the app's data API, resolved before its routes or `web/` | `protocol.md §9.1`, `data-api.md` preamble (found in M9) |
| 67 | `cli.md §5` bound the `PV4xx` rules to apps; nothing said the framework's own pages met them, and the shell shipped `<th>` without `scope`, a focus ring at 1.5:1 and a bare `<ul>` for the settings navigation | The launcher, the settings pages, the error pages and the page frame are held to `PV401`–`PV407` by the framework's own tests over their rendered HTML, since the linter reads templates and those pages have none | `cli.md §5.4` (found in M10) |
| 68 | `PV404`'s "exactly one `<h1>` per view" was not evaluable per file — `animals/views/play.lsp` has none and `_board.lsp` has it | The unit is the page as rendered: a view with its partials inside the frame, or the document a `layout()` owns; a fragment answering htmx is judged by the element it replaces | `cli.md §5.1` (found in M10) |
| 69 | `data-api.md §5` and `app-contract.md §5.2` promised `pv.js` at "~4 KB"; the helper `§5` specifies — the outbox, the reconnecting stream, the async iterator — is 7.5 KB unminified, and there is no minifier in the runtime path | Under 8 KB, unminified, meant to be read | `data-api.md §5`, `app-contract.md §5.2` (found in M10) |
| 70 | `app-contract.md §4.5` had every `id` "holding a ULID", while `protocol.md §4.1` blesses a caller-chosen key and `apps/animals` keys its `cursor` row `'cursor'` | A ULID unless the app keys a singleton itself, with the pointer to `§4.1` | `app-contract.md §4.5` (found in M10) |
| 71 | `apps/hello/README.md`'s `echo >>` example named the next `seq` and said nothing about `lam`, which is what `§4.5` ranks a line by — a hand-written line with a stale `lam` is durable and loses the merge | The next `seq` **and** a `lam` above the log's highest; the node picks both up on the next request (`§4.3`, M9's rescan), which `test_hello_readme_echo_example_is_valid` runs for real | `apps/hello/README.md` (found in M10) |
| 72 | `cli.md §7` gave `restore` a `--from <path>` and never said what the path is, what happens when the backup and the node both hold a log of the same name, or what `--dry-run` predicts with nothing copied; `docs/backup-and-restore.md` only said "copy the folder back" | `--from` is a backup: a `data/` folder or a data root holding one. A log is copied when absent here or when this node's copy is a byte prefix of the backup's, kept when identical or when this node is ahead, and a file that is neither refuses the whole restore before a byte moves (`protocol.md §3.1`, one writer). Snapshots are copied when absent; `local/` and `cache/` are never read. Then the three tiers, per app; `--dry-run` prints the copy plan and the tier as the node stands | `cli.md §7`, `docs/backup-and-restore.md §3` (found in M11) |
| 73 | `cli.md §7`'s `snapshot --verify` "recomputes checksums" without saying whether it also writes a snapshot, or which snapshots it checks | It writes nothing: every existing snapshot of the named apps is checked, a match is recorded as `sys_snapshot.verified_at`, any mismatch is a non-zero exit | `cli.md §7` (found in M11) |
| 74 | `cli.md §4`'s `--scaffold <table>` "reads `schema.sql`" of an app that `new` is in the act of creating, and nothing said whether `new` may touch an existing folder, whether `--from` and `--tier` may disagree, or what `--from` accepts | The schema is the target's own — copied by `--from` or already in the folder — and `--scaffold` is the one form that accepts an existing folder; `new` never overwrites a file on disk, while within one invocation the scaffold's files stand in for a copy's; `--tier` beside `--from` must agree with the copied manifest; `--from` takes an installed slug, a reference app's, or a folder path; a reserved or malformed slug is a usage error; the title is the slug's words | `cli.md §4` (found in M11) |
| 75 | `cli.md §3`'s `dev --app <slug>` named an app and gave the flag no effect — reloading is the host's on every run, so the flag had nothing to switch on | It names the app being edited: `dev` prints its folder and URL, `--open` opens that URL, and an app that did not load is a runtime error carrying the load failure | `cli.md §3` (found in M11) |
| 76 | `cli.md §6` said nothing about what `skill list` prints, where `export` writes without `--out`, whether it overwrites, or what an unknown name does | Name and front-matter description; `skills/` in the working directory; files replaced, so a re-export after an upgrade is the new version; an unknown name is a usage error listing the names shipped | `cli.md §6` (found in M11) |
| 77 | `cli.md §1` listed `--verbose` with a default and no meaning, and did not say where global flags may stand | It widens the report on standard error from what failed to what happened, changing no behaviour; global flags stand anywhere and `--version`/`--help` end parsing where they stand | `cli.md §1` (found in M11) |
| 78 | `skills/privatium-tier3-rust` told an assistant to run `privatium lint --embedded <binary>`, a flag no section of `cli.md` has, and `privatium-tier1-lua` had `dev` printing a LAN QR code, which is Phase 2 | The skills name the commands the spec has — `new`, `dev`, `lint`, `skill export` — and say what `--open` does in this phase | `skills/privatium-tier3-rust`, `-tier1-lua`, `-tier2-web`, `-overview` (found in M11) |
| 79 | `cli.md §5` gave `lint` a `<path>` and never said what one is, what "the node configuration" contributes, whether `--severity` is a floor or a filter, or what the text format looks like | A path is an app folder, a folder of them (searched three deep), or a file inside one; a path with no app is one `PV101`; the configuration's mode decides `PV502` and `PV506` and a configuration that does not load is a runtime error; `--severity` is a floor and exit 3 is anything at or above it; the text line is spelled out | `cli.md §5` (found in M12) |
| 80 | `PV107` had no home in the contract: the rule table said what `schema.sql` may contain and no section of `app-contract.md` did, so the finding had nothing to cite | `§4.5` says `schema.sql` is declarations — `CREATE TABLE`, `CREATE VIEW`, `CREATE INDEX` and comments — and that the linter judges each statement by the actions the engine reports, never by its first word | `app-contract.md §4.5` (found in M12) |
| 81 | `data-dictionary.md §2` said `privatium lint` catches `SUM()` over a `DECIMAL` and `+` on a `DATE`, and no rule in `cli.md §5.1` did | `PV308`, error, over the SQL literals of Lua, templates and JavaScript and the bodies of `CREATE VIEW` | `cli.md §5.1` (found in M12) |
| 82 | `PV307`'s row — "no Lua global assigned at module scope expecting persistence (VMs are pooled)" — predated row 47, after which a module-scope global is the baseline and the two footguns are a handler's global and a mutated load-time table | The row says what `lua-api.md §5` now says: no global assigned in a handler expecting persistence, no load-time table mutated from one | `cli.md §5.1` (found in M12) |
| 83 | The `PV4xx` rows cited no criterion, so a finding of one could name only the rule table | A paragraph after the table names each rule's WCAG 2.2 success criterion and says `PV401`'s icon requirements are `docs/icons.md`'s, which its findings cite | `cli.md §5.1` (found in M12) |
| 84 | `PV105` said "tier-required files present" and nothing about a file that does not parse, which the loader refuses too | Present, and every `app.lua`, `lib/*.lua` and `views/*.lsp` parses | `cli.md §5.1` (found in M12) |
| 85 | `PV206` forbade `innerHTML` with data and no normative section said why or what to do instead — `app-contract.md §5.4` covered inline script and never markup built from a string | `§5.4` says markup built from data is the injection the policy cannot see, names the three sinks, and names `textContent` | `app-contract.md §5.4` (found in M12) |
| 86 | Row 4 put the fixtures at `apps/_lint/{pass,fail}/<rule>/` — but `PV104` compares the slug to the folder name and `PV104` is not a slug, so a pass fixture could never be clean there | `apps/_lint/{pass,fail}/<rule>/<slug>/`, the rule directory holding the app; a rule directory may carry the `config.toml` it is linted under | `cli.md §5.4`, `roadmap.md` (found in M12) |
| 87 | `docs/skills.md §2` said the reference files are "generated at build time; not committed" and named files (`examples/`, per-skill lists) that no generator writes; `§7` named two skills as generated | Every skill's `reference/` is generated by `cargo xtask gen-skill-reference` and committed, the tree names what is written and where each file comes from, and `§7` names the command and the `--check` gate | `docs/skills.md §2, §7` (found in M12) |
| 88 | `PV203`'s row listed seven names; `lua-api.md §5` removes fifteen | The row says "and the rest of `§5`'s closed list", and the linter flags all of them | `cli.md §5.1` (found in M12) |
| 89 | `§5.2` required a "resolvable" `spec` reference without saying what resolves; two rules of the table itself cite `docs/` documents | A reference is `<spec or docs path> [§section]`, the document exists and the numbered heading is in it; `cargo xtask lint-spec-refs` and a core test hold every rule to that | `cli.md §5.2` (found in M12) |
| 90 | `protocol.md §3` showed `local/` holding `state.jsonl` alone, and nothing kept a second `privatium` — a `snapshot --verify` beside a running node — from opening the same logs and minting `seq` beside it | `local/lock`, held exclusively by the process that has the root open, a second refused; `cli.md §1` names which commands take it | `protocol.md §3, §3.1`, `cli.md §1` (hardening) |
| 91 | `cli.md §7` said a log is "copied" when this node's copy is a prefix of the backup's — `fs::copy` truncates the destination first, so a crash mid-copy left the node a shorter log than it had — and nothing held other processes off during the copy | A copy never overwrites: written beside and renamed, or grown by the suffix, each file decided again at the moment it is written, under the root's lock from the plan to the rebuild | `cli.md §7`, `docs/backup-and-restore.md §3` (hardening) |
| 92 | `data-api.md §2` said "atomic: all lines or none" and `lua-api.md §3.3` "every event or none", while a batch was one write of plain lines: a crash that landed a newline-aligned prefix left a batch every reader took for a smaller one, and only a torn last line was detectable | The first line of a batch of two or more carries `"batch": n`; a reader skips a batch that reached the disk short, keeps its lines, continues past them, and the node audits `batch.incomplete` once | `protocol.md §4.1, §4.5`, `lua-api.md §3.3`, `data-api.md §2`, `data-dictionary.md §3.10` (hardening) |
| 93 | `protocol.md §10.6` said "ULIDs make replay idempotent" and `data-api.md §6` "needs no bookkeeping", while a browser's retry of a write that had landed was stamped afresh by the node and superseded any edit made in between; neither said what a replayed edit does to one | Idempotent at the row; whether a write landed is read from the log past the mark the entry was queued at, never remembered; a queued edit whose row moved since is refused and reported rather than written over the newer change (row 101); what a browser does send is stamped as it arrives, since it is not a device (`§10.7`) | `protocol.md §10.6`, `data-api.md §6`, `AGENTS.md` 11 (hardening) |
| 94 | `data-api.md §4`'s `/api/node` named no app, so a page at a solo mount — `/`, for whichever app the node serves — could not tell which app its outbox was for, and `pv.js` keyed the queue by the mount alone | `app` in `/api/node`; an entry carries the app it was queued under and is refused, not replayed, when the mount serves another | `data-api.md §4, §6` (hardening) |
| 95 | `data-api.md §6` dropped every non-2xx replay as "refused", a 503 from a node that was up but failing included; `pv.js` also lost a queued entry to an append made during a replay, since each call re-read storage, lost the queue outright without storage, and — the one that mattered — never replayed at all after an empty flush at load | A 429, a 5xx or an unreachable node keeps the entry and ends the pass; one queue in memory, mirrored to storage; the helper runs under `node --test` in CI | `data-api.md §6` (hardening) |
| 96 | `data-api.md §5` and `app-contract.md §5.2` promised `pv.js` under 8 KB (row 69); the landed check, the app check and the retention rule are 1.9 KB more | Under 10 KB | `data-api.md §5`, `app-contract.md §5.2` (hardening) |
| 97 | `app-contract.md §5.4` said `cross_origin_isolated` "is honoured only for the solo app" and `§9.3` listed the headers of every response; no response carried COOP or COEP, so the permission was accepted and did nothing | The two headers on every response of the origin in solo mode; `§9.3` says so | `app-contract.md §5.4`, `protocol.md §9.3`, `docs/frameworks.md §5.4` (hardening) |
| 98 | `app-contract.md §4.6` still said "globals are per-VM and do not persist" after row 47 made a handler's global request-scoped and `app.lua`'s the baseline; `README.md` said no code existed; `protocol.md` called itself pre-implementation; the Tier 1 skill promised a LAN QR code; `AGENTS.md`, `docs/skills.md §4` and `skills/README.md` said every skill ends with `privatium lint` while the Tier 3 skill ended with `skill export` | Each says what is so | `app-contract.md §4.6`, `README.md`, `protocol.md`, `skills/`, `AGENTS.md`, `docs/skills.md §4` (hardening) |
| 99 | `protocol.md §4.1` said nothing about an append that fails part-way: the writer advanced nothing and would have minted the same `seq` again, or appended after a torn line | A writer whose write or flush fails MUST NOT append again until it has re-read its file; the next `seq` is what the file ends with, and a file ending mid-line stays closed | `protocol.md §4.1` (hardening, second round) |
| 100 | `data-api.md §6` let an entry queued before the helper knew which app a solo mount served replay into whichever app owned `/` later | Refused and reported; a host-mode mount names its app in the path, so only a solo page loaded with the node unreachable and nothing cached is affected | `data-api.md §6` (hardening, second round) |
| 101 | Row 93 had a queued edit replayed after another device edited the row win by arrival — honest, and the wrong default for a single owner, who was not told a newer edit had been overwritten | The row moved since the write was queued: the whole entry is refused and reported as a conflict, never written over the newer change. The owner's decision, after the review's re-audit | `protocol.md §10.6`, `data-api.md §6` (hardening, second round) |
| 102 | `cli.md §2` and `lua-api.md §7` had `--open` print a QR code, and `apps/sketch/README.md` promised pairing, discovery and sync — none of which a Phase 1 build has | Each says what a Phase 1 build does and which phase the rest arrives with | `cli.md §2`, `lua-api.md §7`, `apps/sketch/README.md` (hardening, second round) |
| 103 | `app-contract.md §2.3` depended on `privatium-core` and then called `privatium::Node::open`, a crate the dependency line does not name; the Tier 3 skill did the same and listed `default_data_dir`, `ulid()`, `now()`, `Decimal`, `query_one`, `get_row` and `events_since`, none of which `§6` lists or the crate has | `privatium_core::` throughout; the skill's table is `§6`'s and its skeleton is `examples/embedded.rs`; a Rust caller mints ids with `new_ulid` | `app-contract.md §2.3`, `skills/privatium-tier3-rust` (found in M13) |
| 104 | `§2.3` appended to and queried `myapp` with nothing saying how the node knew the app existed or where its tables came from — a folder is what `§8` loads, and an embedder's binary has none | `open_app(slug, schema)`: the app's `schema.sql` text inline, empty for a document store; the log, the cache and the stream under `data/<slug>/` as a folder's, with no mount and no index row; `§6` gains the row and the example shows the call | `app-contract.md §2.3, §6`, `data-dictionary.md §3.4` (found in M13) |
| 105 | `§2.3`'s `query` took SQL alone, so a value from outside could reach the statement only by being formatted into it — the injection `§7` sandboxes against — while the skill's took a third argument the spec never gave | `query(app, sql, params)`: positional `?` bound as `data-api.md §1` binds them, never interpolated, a mismatched count refused | `app-contract.md §2.3, §6` (found in M13) |
| 106 | `§6` listed discovery, pairing and sync and said nothing about a build without them; a method returning `Ok` from a no-op is what this plan's M13 forbids, and an absent one is what a reader writes their own of | A build without an area keeps the method and answers with a typed error naming the phase — `Error::Unimplemented` — and MUST NOT return success | `app-contract.md §6` (found in M13) |
| 107 | `§2.3` had an embedder wrap their own router in `auth_layer`, and the layer read a `Peer` extension only the framework's own adapter inserts — so on an embedder's router every caller read as "this process" and nothing was ever refused | The layer reads axum's `ConnectInfo<SocketAddr>` too, which `into_make_service_with_connect_info` attaches; `§2.3`'s example shows that call | `app-contract.md §2.3, §6` (found in M13) |
| 108 | `skills/privatium-tier3-rust/reference/api.md` called itself `Node`'s public methods at this version and listed one file's `impl` block, so `load_apps`, `append` and their neighbours in `app/mod.rs` were absent from the pinned reference; and it said the Phase 2 methods were absent | Every `impl Node` block under `src/`, and what the four methods do | `skills/privatium-tier3-rust/reference/api.md`, `crates/xtask` (found in M13) |
| 109 | `app-contract.md §2.3` and `§6` had `auth_layer` read the peer from `ConnectInfo` and said nothing about a request with none. The layer allowed it, as the one over `handle` must for its in-process callers, so an embedder's router served without `into_make_service_with_connect_info` admitted every caller | The layer an embedder wraps their router with refuses a request whose peer it cannot see, naming the missing call; an in-process caller inserts `Peer`; `Handler` applies its own permissive copy to `handle` | `app-contract.md §2.3, §6` (hardening, third round) |
| 110 | `app-contract.md §4.5` and `PV107` permitted `CREATE INDEX`, and nothing created one in the cache — the schema kept tables and views only — while a `UNIQUE` constraint or index was enforced by `validate` within one batch and nowhere else | Declared indexes are recreated on every rebuild; `UNIQUE` beyond `id`'s primary key is refused at load and by `PV108`, because two devices' logs may both claim a value and `protocol.md §4.5` keeps both rows | `app-contract.md §4.5`, `cli.md §5.1` (hardening, third round) |
| 111 | `data-api.md §6` keyed an outbox entry by its app alone and kept the queue as one list under one key: a solo `/` that changed apps, or another data root on the same port, was replayed into; two offline pages replaced each other's list; and `pv.get` never moved the mark, so an edit of a row the page had read was refused as a conflict | An entry carries the node's `id` too, storage holds one key per entry, a replay asks `/api/node` again before it sends, every POST names its `node` and `app` and the node refuses a mismatch itself (`§2`), and `pv.get` moves the mark | `data-api.md §2, §6` (hardening, third round) |
| 112 | Rows 69 and 96 set the helper's cap at 8 then 10 KB; the node binding, the per-entry storage and a ULID that stays monotonic within a millisecond are 1.9 KB more | Under 12 KB | `data-api.md §5`, `app-contract.md §5.2` (hardening, third round) |
| 113 | `protocol.md`'s status line still said Phase 1 was in progress; `apps/README.md` and `README.md` had the reference apps shipping inside the binary, which M13 decided against; the README credited the later phases' libraries as in use; `apps/sketch`, `skills/privatium-games` and `crates/privatium/src/run.rs` described a sync, and a snapshot read under the lock, that the code does not have; ADR 0004 carried an open task over a claim no source supports | Each says what is so | `protocol.md`, `README.md`, `apps/`, `skills/`, `run.rs`, `docs/decisions/0004` (hardening, third round) |
| 114 | `protocol.md §10.6` and `data-api.md §6` had the page read each row's events past its mark and then POST unconditionally, so a write from another page could land between the read and the POST; the mark meant "the node had this much", not "the page saw this much", so an unrelated `query` moved it past an edit to the very row being edited; and `pv.get` never moved it, so an edit of a row the page had read was refused as a conflict | The node judges a replay under the lock in which it appends: the POST carries `since`, the mark, and per event a `base` — the rank of the row's winner as the page saw it, kept for every row read through `get`, `events` or the stream and every row the page wrote. A copy past the rank is landed and appends nothing, anything else past it is 409 naming the row, nothing is fresh; a body with neither is unconditional | `protocol.md §10.6`, `data-api.md §2, §5, §6` (hardening, fourth round) |
| 115 | `data-api.md §6` said a landed event is "compared as it was sent" and that a typed app's normalized value "lands as the same row again" — under row 101's rule it would have been a false conflict | The node compares the event as it would store it, so normalization tells nothing apart | `data-api.md §2` (hardening, fourth round) |
| 116 | `data-api.md §2`'s response carried `lam` and `ids`, so a page knew the mark of what it wrote but not its rank | `ts` and `dev` too — one instant, this node | `data-api.md §2` (hardening, fourth round) |

Defect 11 was found during M1 rather than while writing this plan, which is the rule in
the last paragraph of this section working as intended. It could not be coded around: the
`sys_device` self-row must be keyed by Node ID, or every later update to it would mint a
fresh ULID and `§4.5` would materialize a second device row instead of amending the first.
It does not touch sync — `§10.1` is a set union over `(dev, seq)` and never reads `id`, and
`§10.6` depends on a retry carrying the *same* `id`, not on its shape.

Two of these are additions rather than corrections and deserve to be called out as such:
**`PV506`** did not exist before, and **`--version`** widens the CLI surface `spec/cli.md §10`
otherwise keeps deliberately narrow. Both were added because §2 depends on them; reject
either and §2.1 or §2.6 needs rewriting rather than quietly proceeding.

**The rule this section establishes for the rest of the project:** when implementation
reveals a spec defect, fix the spec in the PR that found it. Do not accumulate a defect list
and do not code around it. `docs/skills.md §7` already makes an unreflected spec change an
incomplete change; this is the same principle one step earlier.

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
│   │   ├── store/            SQLite materialize, snapshot, restore (ADR 0006)
│   │   ├── app/              app.toml, loader, lifecycle, permissions, CSP
│   │   ├── lua/              mlua host, sandbox, pool, pv module, lsp/
│   │   ├── wire/             Request/Response, core::handle, router (ADR 0003)
│   │   ├── http/             shell, settings, data API, SSE, auth_layer
│   │   ├── lint/             rules, fixer, json output
│   │   └── sys.rs            sys_* event helpers
│   ├── assets/icons/         vendored twbs/icons + LICENSE + VERSION (M6)
│   ├── examples/embedded.rs  the 30-line Tier 3 proof
│   └── tests/                spec-named integration tests
├── privatium/                the binary; the argument grammar (by hand), subcommands, axum adapter, terminal output
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
| Async | `tokio` | multi-thread runtime; from M9 the core takes `sync` (the per-app broadcast, the stream's channel), `time` (the 30 s ping) and `macros` (`select!` in the stream's pump) |
| Streams | `futures-core` | The `Stream` trait `Body::from_stream` takes, for M9's SSE body — a channel receiver polled as a stream. The trait crate alone, already in the graph beneath axum, so no new compile unit; not `tokio-stream` (a new crate for one wrapper) and not the `futures` facade. |
| Lua | `mlua` | features `lua54`, `vendored`, `send` |
| SQL | `rusqlite` | features `bundled`, `functions`, `window`, `collation`, `hooks`, and from M7 `column_metadata` (a result column reports the table and column it originates in, which is how `pv.query` finds a declared type); was `duckdb` until ADR 0006 |
| Crypto | `ed25519-dalek`, `sha2`, `hmac` | no session crypto in Phase 1 |
| IDs | `ulid` | Crockford Base32, 26 chars |
| Serde | `serde`, `serde_json` | `preserve_order` **off** — see below |
| Config | `toml`, `figment` or hand-rolled | manifest + config.toml |
| Watch | — | *Was `notify` + debounce for `privatium dev`. M8 decided a stat per request in the core instead — `refresh_app` already stats the log per request, and the code files and templates joined it — so nothing watches and nothing is taken; the workspace rows were removed.* |
| Lua AST | `full_moon` | every Lua rule of the linter, and a template's compiled chunk; `default-features = false` (no serde), `lua54`. M12: 2.2.0 parses every reference app and the corpus, so R4's fallback was not needed |
| CLI | — | *Was `clap` (derive) and `owo-colors`. M11 took neither: the surface is eight commands and twenty flags fixed by `spec/cli.md`, whose synopsis lines are the help text, so `std::env::args_os` and three hundred lines (`crates/privatium/src/cli.rs`) parse it — as `xtask` already did — with no proc-macro compile and nothing for the help to drift from. Colour is unspecified. The workspace rows were removed.* **No `qrcode`** — QR is pairing, which is Phase 2 |
| Errors | `thiserror` in core, `anyhow` in the binary | `AGENTS.md` |
| Embed | `include_dir` | icons, shell assets, `pv.js` |
| Time | `jiff` | `default-features = false`; `ts` is RFC 3339 UTC to the millisecond (`§4.1`), `§4.4` compares against it, M3 stores it as text. Added in M1 — this row was missing. |
| Paths | `directories` | `BaseDirs::data_local_dir()`, **not** `data_dir()`: on Windows the latter is `%APPDATA%`, which roams, and a roamed `node.key` means two machines with one Node ID. Added in M1. |

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
- `xtask spec-drift`: warns when `spec/` has changed since `skills/` was last reconciled
  (R8). A hash manifest, not a diff — the generator that could diff is M12 (it was M13
  until M11 moved it), and this is replaced by it there.
- CI matrix: Linux, macOS, Windows. `fmt`, `clippy -D warnings`, `test`.
- Lint config denying `clippy::unwrap_used` and `clippy::expect_used` in
  `privatium-core`, allowed in `tests/` and in `main()` startup only.
- `deny.toml`: fail on GPL-2.0-only and non-commercial licences (ADR 0001 is a licence
  decision; make it mechanical). ADR 0004 §5 explains why this matters more than it looks.
- R1 and R2 build gates: `privatium-core` really links the bundled engine and vendored Lua,
  and CI runs the release binary and reports its size on all three platforms. The
  `ed25519-dalek`/`rand`/`sha2` trio is compiled here too, so M1 does not open with a
  `rand_core` version conflict.

**Done when:** CI is green on all three platforms and `xtask header-check` fails when a
header is deleted. Not "green on an empty workspace" — R1 requires the bundled engine
build to be proven here, and a workspace with no dependencies would prove nothing.

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
  repaired, reordered, or rejected (`protocol.md §4.1`). Gap rejection is a sync concern
  (`§10.2`) and arrives in Phase 3.
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

> *Written for DuckDB. ADR 0006 (after M6) moved the engine to SQLite; the section is
> kept as the record of what was built, and §2.7 says what changed.*

- Privileged DuckDB connection on `cache/<slug>.duckdb`; app-facing connection configured
  per `app-contract.md §7` with `lock_configuration = true` **last**.
- Learn what `schema.sql` declares from **DuckDB's own catalog**, not from a parser we
  wrote. Execute the file into a throwaway in-memory instance that is sealed first —
  external access off, autoload off, `lock_configuration` on — then read `duckdb_tables()`,
  `duckdb_columns()`, `duckdb_constraints()` and `duckdb_views()`, filtered to
  `schema_name = 'main' AND NOT internal`. That gives names, exact types, `NOT NULL` and
  `CHECK` from the engine that will execute them, and it is neither a regex nor a
  third-party SQL crate.

  **Not `json_serialize_sql()`**, which an earlier draft of this plan named. It refuses
  every statement that is not a `SELECT` — handed DDL it returns
  `{"error":true,"error_message":"Only SELECT statements can be serialized to json!"}` —
  so it cannot read a `schema.sql` at all. `test_r1_duckdb_json_is_statically_linked` pins
  that, so a later DuckDB lifting the restriction is noticed rather than assumed.

- **`duckdb = { features = ["bundled", "json"] }`.** `libduckdb-sys` compiles an extension
  only when its cargo feature is on, so `bundled` alone has no `read_json()` and nothing
  below is possible. The feature is also what satisfies `AGENTS.md`'s "statically linked":
  it defines `DUCKDB_EXTENSION_JSON_LINKED`. Autoload is **not** off by default — that
  build sets `DUCKDB_EXTENSION_AUTOLOAD_DEFAULT=1` — so every connection turns it off
  explicitly. `parquet` waits for M4.
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
  WHERE app = 'hello' AND tbl = 'profile'
    AND seq IS NOT NULL AND lam IS NOT NULL AND id IS NOT NULL
    AND (try_cast(ts AS TIMESTAMPTZ) IS NULL
         OR try_cast(ts AS TIMESTAMPTZ) <= TIMESTAMPTZ '<now + 24h>')
), ranked AS (
  SELECT *, row_number() OVER (
    PARTITION BY id ORDER BY lam DESC NULLS LAST, ts DESC NULLS LAST, dev DESC NULLS LAST
  ) AS rn
  FROM ev
)
SELECT id,
       CAST(json_extract_string(d, '$."display_name"') AS VARCHAR) AS display_name
FROM ranked
WHERE rn = 1 AND op = 'put';
```

Four things in that `WHERE` are not decoration, and an earlier draft of this sketch had
none of them:

- **`app = 'hello'`.** §4.5 step 1 is "every event where `app = A` **and** `tbl = T`". The
  earlier sketch declared the `app` column and then never used it.
- **The NULL guards.** `read_json()` with an explicit `columns` list yields NULL for a
  field it cannot find, so a line that is not an envelope becomes a row of NULLs rather
  than an error, and a row with no `lam` has no business in a causal ordering.
- **The `ts` clause is §4.4.** A future-dated event is still in the file whether or not the
  reader folded its `lam` in, and letting it materialize hands it the row permanently — a
  rejection that only withholds a counter increment is not a rejection. `try_cast` mirrors
  the reader's one mercy: a `ts` this node cannot parse carries no information and is
  *accepted*, because dropping it would be gap rejection by another name. The horizon is
  passed **in** rather than read from the clock, so §2.5's two paths cannot disagree merely
  because time passed between them.
- **`op = 'put'`**, not "anything that is not a `del`". §4.5 step 4 says "otherwise the row
  is its `d`", which read literally would treat an unknown future `op` as a full put.
  Within `pv/1` there are exactly two ops and the readings agree, so this is a comment at
  the call site rather than a spec edit — tightening the wording would be speculating about
  `pv/2`.

Per-column extraction is type-directed, not one expression for everything: scalars go
through `json_extract_string` and cast the text, which is what unwraps §2.1's string
encoding for `DECIMAL`/`BIGINT` (and rescues a client that wrongly sent a JSON number);
`VARCHAR[]` and other structured types go through `json_extract` and cast the JSON value.

- Column list generated from `schema.sql`. `NOT NULL` and `CHECK` are extracted as
  **metadata** and the materialized table deliberately carries no constraints — they are
  enforced before append (`data-api.md §2`), which is M7's and M9's call site, not M3's.
  There is no append caller in M3 to validate.
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

> *Written for DuckDB. ADR 0006 (after M6) moved the engine to SQLite; the section is
> kept as the record of what was built, and §2.7 says what changed.*

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

- Discover `apps/` under the data root **and** the repo's `apps/` in dev; skip `_*` — this
  covers both `_sys` (§2.6) and the lint fixture corpus.
- `app.toml` parse and validate: required keys, slug regex, all ten reserved slugs of
  `protocol.md §1.1`, directory-name match, `api` ≤ supported.
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
- **Solo-mode precedence** per `protocol.md §9.1`: framework prefixes win; a shadowed app
  route is a load-time warning, and `PV506` catches it in CI.
- Security headers of `§9.3` on every response; `Cache-Control: no-store` on anything
  carrying app data.
- `GET /api/v1/health` returns `{"v":1,"id":"..."}` only. `GET /api/v1/manifest` returns
  node ID, display name, app index, and the `pair` flag — no row counts, no activity
  timestamps, no app content.
- `/skills/<name>.md` and `/skills/bundle.zip` (`spec/cli.md §6`).
- `auth_layer` per §2.2. Loopback bind per §2.1.
- Shell: server-rendered HTML + HTMX, Bootstrap Icons inlined via `include_dir`. No
  bundler, no `node_modules` in the runtime path.
- Vendor `twbs/icons` v1.13.1 in full into `assets/icons/`, with `LICENSE`, `VERSION`, a
  `VENDOR.md`, and the attribution in `NOTICE`. Not a build-time subset: apps are
  installed at runtime and declare their own icon (`docs/icons.md`).
- `xtask icons-verify`: every icon name referenced anywhere in `apps/` and the shell, and
  every name in the `docs/icons.md` vocabulary table, exists in the vendored set — per
  `docs/icons.md` and `PV503`. This was M0 in an earlier draft, where there was neither a
  vendored set to check against nor any HTML that rendered an icon.
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

- **`panic` stays at the default in `[profile.release]`, and M7 is where that is
  confirmed rather than assumed.** A limit abort must fail the request and leave the node
  and the next request untouched; `panic = "abort"` may foreclose whatever mechanism does
  that. Decide it here, with the limit tests in front of you — not in a profile table.
  *Decided in M7, against the running release binary: it forecloses it entirely. mlua
  raises a Lua error out of a Rust callback by unwinding through the callback frame, and
  with `panic = "abort"` the first limit trip aborted the whole node ("panic in a function
  that cannot unwind"). With the default the same request is a 500 and every later request
  answers. The default stays; `Cargo.toml` and `AGENTS.md` say why.*

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
  *Decided in M8: the table is the host's behaviour on every run, not `dev`'s. A stat of
  `app.toml`, `app.lua`, `schema.sql`, `lib/**` and `views/*` per request — beside the stat
  of the log `refresh_app` already made — decides, under the node lock and before a VM is
  checked out, so R3 holds by construction and there is no watcher thread; `notify` was
  not taken. A save that does not load is the error page until the next save loads;
  `dev` (M11) is the front door and adds its flags. `static/` beneath a Tier 1 mount is
  served here too, host mode and solo (`§3` rows 51–52) — nothing else rendered animals.*

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
  *Decided in M9: `$name` in a `CREATE VIEW` is rewritten to `pv_param('name')` at load,
  because SQLite forbids a parameter in a view (`§3` row 57); the API's CSRF story is a
  JSON-only POST plus a refusal of `Sec-Fetch-Site: cross-site`, and no token (row 58);
  `NOT NULL` and `CHECK` run in `Node::append` for every writer (row 63); `sys` is attached
  on every app connection before the authorizer (row 65). The stream is a `tokio::sync::mpsc`
  receiver polled as a `futures_core::Stream` and pumped by a task off the lock — subscribed
  to the app's `broadcast` and reading its backlog under one hold of the lock, so nothing
  lands between the two — with a ping tick that stats the app, so a hand-appended line
  reaches an idle node's streams as a `resync`. `refresh_app` rescans the log when it moved
  behind the tables, so the Lamport counter and this node's own `seq` follow an `echo >>`
  (`spec/protocol.md §4.1`, `§4.3`); they had not since M3. No long-poll fallback.*

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

- `hello`: three routes, one table, two templates, no JavaScript.
- `animals`: recursive SQL, `pv.batch` multi-event writes, stored cursor state, the
  HTMX/`is_htmx` branch **and** the no-JavaScript redirect branch, Alpine CSP build under
  the default CSP, the `_board.lsp` partial swap.
- `sketch`: canvas, `pv.js`, event log as document store, no SQL, no `schema.sql`.
- Accessibility baseline on the shell: labels, heading order, focus, contrast — the `PV4xx`
  rules apply to the framework's own HTML, not only to apps.
  *Decided in M10: the `PV4xx` checker for rendered pages lives in the test suite
  (`tests/common/a11y.rs`), not in `src/lint/` — M12's linter reads templates, and the
  shell and the page frame have none (`§3` row 67); M12 may lift the contrast maths. The
  unit for `PV404` is the page as rendered (row 68). The shell's focus ring is the
  scheme's accent — navy on light, maize on dark — and a control's border is the muted
  text colour, so both clear 3:1 in both schemes; the launcher no longer dims an
  unavailable app; a `prefers-reduced-motion` guard; `<th scope>` on both tables; the
  settings sub-navigation inside `<nav aria-label>`; and htmx told
  `includeIndicatorStyles:false`, since the `<style>` it otherwise injects is refused by
  the default CSP with a console error on every page (confirmed in Edge: an inline
  `<style>` under this policy is refused with "Applying inline style violates …
  'default-src 'self''"). Headless Edge over the DevTools protocol also found that the
  Alpine CSP build had never initialised: Alpine's CDN builds call `Alpine.start()` in a
  microtask as soon as their script runs and dispatch `alpine:init` right then, so
  `animals.js`, loaded after Alpine, registered its components too late and every
  `x-data` was an "Undefined variable" — invisible to a test through `handle`, since no
  test runs the page's scripts. `_assets.lsp` now loads `animals.js` first, both `defer`,
  and `test_animals_end_to_end` holds the order. `animals` gained
  `sample/seed.jsonl` and a no-JavaScript path: `static/nojs.css` linked from a
  `<noscript>` reverts `x-cloak` and hides the `pv-js-only` toggles, so the reset form
  and the question paths are reachable with scripts off — an external sheet because an
  inline `<style>` would be blocked; `PV402` stays as written and the teach radios gained
  `id`/`for` inside their wrapping labels. `sketch` sizes its canvas in CSS and matches
  the backing store to `clientWidth × devicePixelRatio` (the old `innerWidth` sizing drew
  past the viewport on every HiDPI display), keeps the viewport zoomable, labels the
  canvas, announces the current colour with `aria-pressed`, and gained an `<h1>` and a
  `<main>`. `hello`'s empty state is an `<h1>`, and its README's `echo >>` line names a
  `lam` as well as a `seq` (row 71). `pv.js` is specified at under 8 KB (row 69).*

**Tests:** end-to-end per app: load, render, write, reload, verify against the log. Plus
`test_animals_works_with_javascript_disabled` and
`test_hello_readme_echo_example_is_valid` (parse the README's own command, run it, assert
gapless).
*Landed in M10 as `tests/reference.rs`: `test_hello_end_to_end`,
`test_hello_readme_echo_example_is_valid`, `test_animals_end_to_end`,
`test_animals_works_with_javascript_disabled`, `test_sketch_end_to_end`,
`test_spec_cli_5_pv4xx_shell_pages`, `test_spec_cli_5_pv4xx_app_frame_and_reference_views`,
`test_spec_cli_5_pv406_declared_tokens_meet_contrast`.*

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
- Remove the M0 engine-version placeholder from `main()`; the link gate moves to a
  `#[used]` reference or to the CI size check. Bare `privatium` runs a node from here on,
  and the comment in M0's `main()` saying so will not survive the rewrite that replaces it.
  *Done: the placeholder, and the `PRIVATIUM_DEV_SERVE` start M6 kept behind it, are gone;
  CI runs `--version` and `skill list` against the release binary, which is the linkage
  proof now that a bare run would wait forever.*

*Decided in M11: no `clap` (§5). The grammar is `crates/privatium/src/cli.rs`, and the
help text is `spec/cli.md`'s synopsis lines verbatim, which is what
`test_no_undocumented_flags` compares in both directions. The generator lives in the core
(`app::scaffold`) and returns files; the binary writes them and refuses to overwrite. The
backup import behind `restore --from` is the core's too (`backup::Plan`, built then
applied, so `--dry-run` and the real run share one decision). `Node::config_mut` carries
`--port` and `--solo` for one run. Nothing had called `Node::maintain` since M4 — M6's
"server loop weekly" never existed — so the run loop now maintains `_sys` and every loaded
app at start and every 24 hours, which is what makes `cli.md §7`'s "snapshots are written
automatically" true. `--open` is the platform opener (`cmd /C start`, `open`, `xdg-open`).
`lint` parses its flags and says it is M12's, as `pair` and `firewall` say their phases.
The boolean input the scaffold emits is a checkbox inside a `fieldset`/`legend`, because
`PV403` as `tests/common/a11y.rs` reads it takes a lone checkbox for a group of one. §3
rows 72–78 are the spec gaps found.*

**Tests:** `test_cli_exit_codes`, `test_new_from_hello_rewrites_slug_and_title`,
`test_scaffold_output_passes_lint`, `test_no_undocumented_flags` (compare the help output
against the flags named in `spec/cli.md`).
*Landed in M11 as `crates/privatium/tests/cli.rs`, against the built binary:
`test_spec_cli_1_version_qualifies_protocol`, `test_cli_exit_codes`,
`test_no_undocumented_flags`, `test_spec_cli_2_runs_a_node_on_loopback`,
`test_spec_cli_3_dev_names_the_app`, `test_spec_cli_4_new_each_tier_loads`,
`test_new_from_hello_rewrites_slug_and_title`, `test_spec_cli_6_skill_list_and_export`,
`test_spec_cli_7_snapshot_and_verify`, `test_spec_cli_7_restore_from_backup_reports_tier`,
`test_spec_cli_7_restore_refuses_a_diverged_log`,
`test_spec_cli_8_9_pair_and_firewall_parse_and_refuse`, `test_spec_cli_10_absent_commands`;
and as `crates/privatium-core/tests/scaffold.rs`, through `core::handle`:
`test_scaffold_output_passes_lint` (the CRUD round trip with every page held to the
`PV4xx` checker), `test_spec_cli_4_every_tier_loads`,
`test_spec_cli_4_from_copy_rewrites_slug_and_title`.*

---

### M12 — The linter

> *Written for DuckDB. ADR 0006 (after M6) moved the engine to SQLite; the section is
> kept as the record of what was built, and §2.7 says what changed.*

Ships in Phase 1 because it is what makes `skills/` enforceable rather than advisory.

- Implement every rule in `spec/cli.md §5.1`: `PV101–107`, `PV201–208`, `PV301–307`,
  `PV401–407`, `PV501–506`.
- Lua rules over a `full_moon` AST, not regex. HTML/template rules over the LSP parse tree
  from M8 — the linter reuses the compiler's front end rather than growing a second one.
- SQL rules: `PV106` (every table has `id VARCHAR PRIMARY KEY`) reuses M3's catalog
  introspection, which already refuses a table with no `id`. **`PV107` is unresolved and
  is M12's to settle.** "Contains only `CREATE TABLE`, `CREATE VIEW`, `CREATE MACRO`,
  `COMMENT ON`" is statement *classification*, and the catalog cannot answer it — an
  `INSERT` leaves no catalog trace. `json_serialize_sql()`, which an earlier draft named
  here, only handles `SELECT` (see M3). DuckDB exposes no classifier to safe Rust:
  `duckdb_prepared_statement_type` is raw FFI that `duckdb-rs` does not re-export, and
  `duckdb_prepared_statements()` carries no statement type. The candidates are a scoped
  `unsafe` FFI call, or executing into the sealed instance M3 already builds and asserting
  that nothing but tables, views and macros appeared and that every table is empty. Decide
  it in M12, with the fixture corpus in front of you.
- `PV502` (`cross_origin_isolated` only in solo mode) needs the solo-mode knowledge from
  M6; wire it, do not stub it. `docs/frameworks.md §5.4` explains why, and the
  `duckdb-wasm` note there is the case that will actually trip someone.
- `--format json`: every finding carries `id`, `severity`, `file`, `line`, `message`,
  `fix`, and a **resolvable** `spec` reference. Add an `xtask lint-spec-refs` that opens
  every referenced section and fails if one does not exist.
- `--fix` applies only mechanical corrections: literal mount path → `url()`, missing
  `focusable="false"`. Never SQL, never Lua control flow.
- Fixture corpus at `apps/_lint/{pass,fail}/<rule>/`, one of each per rule, and a
  meta-test that **fails if a rule has no fixture pair**.
- `xtask gen-skill-reference`: generate `skills/*/reference/*.md` from the crate and the
  spec, and fail CI on drift, per `docs/skills.md §7`. *Moved here from M13 after M11:*
  the linter is the milestone that reads every app file with a rule attached, so it is the
  one that will edit `spec/` and `skills/` most, and a drift check that can say *what*
  drifted is worth more during that work than after it. Every spec edit made in M3, M5,
  M6, M7, M9, M10, M11 and M12 itself must be reflected in the generated reference or this
  fails, which is the intent. It replaces M0's `xtask spec-drift` hash manifest (R8), and
  `FAIL_ON_DRIFT` in `crates/xtask/src/spec_drift.rs` flips to true in the same change —
  or the file goes, if the generator's own check covers it. `docs/skills.md §7` names the
  Tier 1 and Tier 2 reference sections as the generated ones; the generator decides what
  the others hold and says so in `skills/README.md`.

*Decided in M12: `crates/privatium-core/src/lint/` is the module, behind no feature
flag, with the rule table (`RULES`: id, class, severity, what the linter reads, the
section every finding cites) as the one place a rule is defined. Lua is judged over a
`full_moon` tree — `default-features = false`, `lua54`; it parses every reference app and
the whole corpus, so R4's fallback was not needed. A template is judged through M8's own
front end: `lsp::scan` is the tokenizer the compiler now shares, the *compiled chunk* is
parsed with `full_moon` so a `<? if ?>` is an `If` and each branch a state of the page,
and the HTML between tags is synthesized line-aligned with the `.lsp` — `icon()` and
`csrf()` standing in for what they emit — so every element finding names the author's
line. `PV404`'s unit is the page as rendered (`§3` row 68): a `pv.render`ed view with
its `render()` partials inlined, or the layout's document with `content` placed; a view
that is also a partial is a fragment and is judged where it lands; `<h1>` is counted as
a (min, max) over branches and loops, and heading order is a set of possible previous
levels carried through the branches. `PV107` is settled for SQLite the way `§2.7`
suggested: `schema.sql` is split into statements by a literal- and comment-aware scanner
(trigger bodies included), each is prepared under a recording authorizer, and the
statement is classified by the actions SQLite reports — `CREATE TABLE/VIEW/INDEX` plus
the catalog bookkeeping those emit are declarations, and a row written, an object
dropped or altered, a trigger, a temp object, a pragma, a transaction or a bare `SELECT`
is named at its line. `PV106` asks `pragma_table_info` for `id`'s type and primary key.
`PV308` was added (`§3` row 81) for the two rules `§2.7` said the engine made necessary.
Tier 2 JavaScript is lexed — strings, template literals, regexes, comments — not parsed,
which serves `PV201`, `PV206`, `PV207`, `PV301`, `PV302`, `PV304`, `PV305`, `PV306`,
`PV504` and `PV505` and no more. `tests/common/a11y.rs` kept the rendered-page document
checks and now takes the tree, the element rules and the contrast maths from `lint::html`
and `lint::css`. The corpus is `apps/_lint/{pass,fail}/<rule>/<slug>/` (`§3` row 86); a
rule directory may hold the `config.toml` it is linted under, which is how `pass/PV502`
runs solo. `--fix` carries an `Edit` on the finding — byte range and replacement — and
the two mechanical cases are the only ones that ever get one; the CLI applies, re-lints,
and reports what remains. `xtask gen-skill-reference` writes every skill's `reference/`
from the crate (`lint::RULES`, `lua::SURFACE` held to `pv::install` by a unit test, the
sandbox's removed names, the limits, `Permissions::widenings`, the framework prefixes,
the reserved slugs, the icon set, `Node`'s signatures, what `pv.js` exports) and from the
spec by copying numbered sections whole; `--check` fails naming the file that drifted,
and the anti-patterns references are the corpus itself, wrong beside right. `spec_drift.rs`
and its hash manifest are gone. `xtask` depends on `privatium-core` for that. `§3` rows
79–89 are the spec gaps found.*

**Tests:** `test_lint_rule_<id>_passes` and `test_lint_rule_<id>_fails` generated over the
corpus; `test_every_rule_has_fixtures`; `test_reference_apps_lint_clean`;
`test_every_finding_has_resolvable_spec_ref`; and `cargo xtask gen-skill-reference
--check` as a CI step that fails when the committed `skills/*/reference/` differs from
what the generator writes.
*Landed in M12 as `crates/privatium-core/tests/lint.rs`: the seventy-two
`test_lint_rule_pv<id>_passes` / `_fails` from a `rule_tests!` list that
`test_every_rule_has_fixtures` holds equal to `RULES`;
`test_spec_cli_5_4_lint_corpus_files_all_belong_to_a_rule`;
`test_reference_apps_lint_clean`; `test_every_finding_has_resolvable_spec_ref`;
`test_spec_cli_5_2_json_findings_carry_seven_fields`;
`test_spec_cli_5_3_fix_is_mechanical_only`; `test_spec_cli_4_scaffold_lints_clean`;
`test_spec_cli_5_1_pv404_unit_is_the_rendered_page`;
`test_spec_cli_5_paths_are_apps_folders_or_files`; in
`crates/privatium/tests/cli.rs`: `test_spec_cli_5_lint_exit_codes_and_formats`; unit
tests in the module for the PV107 classifier, every Lua rule, the JS lexer, the
line-aligned synthesis and the branch arithmetic; and in CI `cargo xtask
gen-skill-reference --check`, `cargo xtask lint-spec-refs`, and the release binary
linting the three reference apps.*

**Roadmap items satisfied:** the three lint bullets.

---

### Hardening after M12

A review of M0–M12 before M13 found ten things the milestones had left short, none of
them a milestone's feature and all of them the kind that surfaces once every layer exists.
Fixed as one PR between M12 and M13, with `§3` rows 90–98 for the spec each one exposed.

- **One process per data root.** `local/lock` (`lock::DataLock`, `File::try_lock` from
  std — `flock` and `LockFileEx`), taken by `Node::open` and held to drop; `restore` takes
  it before the plan and hands it to `Node::open_holding`. `lint`, `new` and `skill` open
  no node. `test_spec_3_1_second_open_of_a_root_is_refused`,
  `test_spec_cli_1_a_running_node_refuses_a_second_command`.
- **Restore never overwrites.** `backup::Plan::apply` decides each log again at the
  moment it writes it: absent → `.part`, synced, renamed; a prefix → the suffix appended
  and synced; ahead or identical → nothing; else refused. Snapshot directories the same
  way. `a_log_grows_by_its_suffix_and_a_moved_log_is_decided_again`.
- **Batches say their length.** `"batch": n` on the first line of a batch of `n ≥ 2`
  (`§3` row 92); `log::batch::incomplete` is the one rule every reader applies —
  `store::events::read_log`, `wire::data::read_lines`, and `reader::recover`, which
  reports a short batch once as `batch.incomplete`. A newline-aligned prefix of a batch
  was undetectable before; a torn last line is still `PartialLine`.
  `test_spec_4_1_batch_marker_on_the_first_line_only`,
  `test_spec_4_1_incomplete_batch_is_skipped_by_replay_and_audited_once`,
  `test_spec_4_1_short_batch_is_served_by_nothing`.
- **The outbox replays.** `pv.js` set `flushing` after an empty replay had already
  cleared it, so nothing ever replayed after a load with an empty queue; the queue is now
  one array in memory mirrored to storage, a 5xx/429/unreachable node keeps an entry, an
  entry carries the mark and the app it was queued under, and a retry is decided by
  reading the row's events past that mark (`§3` rows 93–96). `/api/node` gained `app`.
  The helper is tested for real: `crates/privatium-core/tests/js/pv.test.mjs` under
  `node --test`, a CI step on all three platforms.
- **Templates publish only what loads.** `Templates::prepare` compiles a candidate,
  `Host::reload_views` preloads it in a VM, and only then `publish`; a candidate that
  fails is remembered by stat so the same broken files are not recompiled per request.
  The spec's behaviour — the error page until the next save loads (`cli.md §3`) — is
  unchanged; what changed is that a broken generation is never current. *Decided against
  the review's wording here: serving the last valid generation silently is what
  `§3` row 53 rejected.* `test_hot_reload_template_next_request` holds the generation.
- **Snapshots off the lock.** `Store::snapshot_job` reads the log (fast, under the lock)
  and `SnapshotJob::write` writes the files (slow, with no lock) — *second round:* the
  read moved off the lock too; the job takes each segment's length under the lock and
  reads that prefix without it; `Node::snapshot_due`,
  `record_snapshot`, `snapshot_retention`, `record_pruned` are the pieces the run loop
  uses, and `Node::maintain` composes them for a caller nobody waits on.
  `test_spec_5_snapshot_job_describes_the_moment_it_was_read`.
- **Directory entries are flushed** where the platform can (`durable::sync_dir`; a no-op
  on Windows, which has no such call and journals its metadata): a new log, the identity
  files, `local/state.jsonl`'s rename, a snapshot's files and rename, every restore copy.
- **`cross_origin_isolated` is honoured**, not only accepted: COOP and COEP on every
  response of the origin in solo mode (`§3` row 97).
  `test_spec_app_contract_5_4_cross_origin_isolated_headers_in_solo_mode`.
- **The documents say what is so** (`§3` row 98).

*Second round, after the review's re-audit of the first:* a failed append closes the
writer until the file is re-read — `Writer::poisoned`, and `AppLog` re-reads on the next
append, continuing whenever the file ends on a line boundary and refusing over a torn
line (`§3` row 99; `a_write_that_fails_part_way_closes_the_writer`,
`a_closed_writer_reopens_from_an_intact_file`,
`a_closed_writer_stays_closed_over_a_torn_line`). The snapshot job takes a length per log
segment under the lock and reads that bounded prefix with no lock held
(`events::read_log_upto`; `test_spec_5_snapshot_writes_while_the_log_grows` appends two
hundred lines while a snapshot is written on another thread). `pv.js` refuses an entry
queued before the app at a solo mount was known, and refuses a conflict — the row moved
since the write was queued — rather than writing over the newer change, which is the
owner's decision (rows 100–101; eleven tests under `node --test`). The last Phase 2
claims in `cli.md §2`, `lua-api.md §7` and sketch's README name their phase (row 102).

*Third round, after an outside review of the finished phase:* the incremental apply is one
transaction — `Store::apply_batch`, `BEGIN IMMEDIATE` to `COMMIT`, the watermark moved once
— so a reader on the sandboxed connection sees a batch whole or not at all, and a batch
the cache cannot take clears the watermark, is rebuilt from the log under the lock
`append_batch` already holds, and reaches the stream as a `resync`. Before, each event was
two to four autocommit statements, and a failure part-way left the watermark describing a
cache that did not match the log
(`test_spec_4_5_failed_apply_leaves_no_half_batch_and_a_stale_watermark`,
`test_spec_4_5_a_reader_sees_a_batch_whole_or_not_at_all`,
`test_spec_4_5_append_heals_a_cache_the_apply_could_not_update`). The layer
`Node::auth_layer` hands an embedder refuses a request whose peer it cannot see, naming the
missing `into_make_service_with_connect_info`, while `Handler` keeps a permissive copy for
`handle`'s in-process callers — a router served without connect info admitted everyone
before (row 109). `CREATE INDEX` reaches the cache: the schema keeps the author's indexes
and every rebuild recreates them, and `UNIQUE` is refused at load and by `PV108`, since the
log cannot keep the promise and `validate` had been half-keeping it within a batch (row
110; `test_spec_app_contract_4_5_declared_indexes_exist_in_the_cache`,
`test_spec_app_contract_4_5_unique_is_refused_at_load`). An outbox entry carries the node
it was queued against, storage holds one key per entry so two pages never write over each
other, a replay asks `/api/node` again before it sends anything, every POST names its
node and app and the node refuses a mismatch itself, `pv.get` moves the mark so an edit of
a row the page read is not a false conflict, and a ULID minted in the same millisecond as
the last sorts after it (rows 111–112; fourteen tests under `node --test`,
`test_spec_data_2_post_naming_another_node_or_app_is_refused`). The sketch canvas
captures the pointer for a stroke, so a release off the canvas saves it; on the branch that
followed, the keyboard draws too — a focusable canvas, a crosshair pen the arrow keys move,
Space to put it down and lift it — and a live summary says how many strokes the canvas
holds in which colours, so the reference Tier 2 app is no longer pointer-only or
sight-only. The documents say what is so (row 113).

*Fourth round:* the replay's check moved to the node. A page had read each row's events
past its mark and then POSTed unconditionally — a write from another page could land
between the two — and the mark itself said what the node had, not what the page had seen:
an unrelated `query` moved it past an edit to the very row being edited, and `pv.get`
never moved it, so an edit of a row the page had read was refused as a conflict. The POST
now carries `since`, the mark, and per event a `base` — the rank of the row's winner as
the page saw it, kept for every row read through `get`, `events` or the stream and every
row the page wrote — and `api_append` judges it under the lock in which it appends: a copy
of the event past the rank means landed and nothing is appended, anything else past it
means the row moved and the batch is 409 naming the row, nothing means fresh. A typed
app's event is compared as the node stores it, and the response carries the batch's `ts`
and `dev`, so the page knows the rank of what it wrote (rows 114–116;
`test_spec_10_6_conditional_append_lands_conflicts_or_appends`,
`test_spec_10_6_a_landed_write_is_landed_after_normalization`, sixteen tests under
`node --test`). `pv.js` reads nothing before a replay any more.

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
- *`xtask gen-skill-reference` was here and is M12's now, so the drift check exists while
  the linter edits the spec rather than after. M13 runs it like any other gate and adds
  nothing to it.*
- Fresh-clone check: `cargo build && ./privatium` on a clean machine produces a working
  `hello` at `http://127.0.0.1:8420`.

**Roadmap items satisfied:** the standalone-core bullet and the cross-platform bullet.

*Landed (`§3` rows 103–108):*

- **The `§6` surface, with the signatures the spec's example implies.** `Event` (was
  `Change`) with `put` and `del`; `append` for one event beside `append_batch`, the
  existing batch write renamed; `query(app, sql, params)` on the sandboxed connection with
  the data API's typing, through one `store::query` both now use; `subscribe` handing out
  the app's broadcast receiver; `close` flushing `local/state.jsonl`; `new_ulid` public.
  The four Phase 2 and 3 methods are present with `Result<()>` and return
  `Error::Unimplemented` naming the phase and the section — the CLI's `not_in_this_build`
  shape — never `Ok`. *Present rather than absent because the spec's skeleton then compiles
  against this version and fails naming the phase, which a reader of the skill meets at
  once instead of writing their own.*
- **`open_app(slug, schema)`**, the one call the spec's example needed and did not show
  (row 104): an app with no folder gets its log, its cache and its stream as a folder's
  does, with no mount, no Lua host and no `sys_app` row; `App::dir` became `Option`.
- **`auth_layer` reads axum's `ConnectInfo`** (row 107), so an embedder's own router refuses
  what the framework's adapter refuses.
- **`examples/embedded.rs`**: 26 lines after the header, held to thirty by
  `test_spec_app_contract_2_3_example_is_thirty_lines_of_the_spec_shape`; run on all three
  platforms by `.github/scripts/embedded-example.sh`, which curls its route and sends it a
  foreign `Host`.
- **The fresh-clone check** is `.github/scripts/fresh-clone.sh`: `cargo build`, then the
  debug binary on an empty data directory serves the launcher and `/a/hello/` at 8420.
- **`§7` asserted by name**: `.github/scripts/conformance.sh` runs the named tests with
  `--exact` and fails when fewer ran than were named.
- **One binary per platform**: the release build is uploaded as a workflow artefact named
  by its target triple, and a `v*` tag publishes the three as a GitHub release with `gh`.
  No installer, no third-party release action. *Decided against embedding the reference
  apps in the binary:* `data-dictionary.md §3.4` gives "the package's folder at install"
  to packaging, which is Phase 6, and the fresh-clone check is from a checkout, where
  `apps/` is beside the binary. A bare binary starts with an empty launcher and
  `privatium new --scaffold`.
- **The generator reads every `impl Node` block** (row 108); a defect in what the
  reference claimed, not a new gate.
- Tests in `crates/privatium-core/tests/embedded.rs`:
  `test_spec_app_contract_2_3_open_app_append_query_with_no_folder`,
  `test_spec_app_contract_2_3_embedded_app_survives_restart_and_a_cache_delete`,
  `test_spec_app_contract_2_3_open_app_refuses_bad_slugs_and_a_folder_collision`,
  `test_spec_app_contract_6_subscribe_sees_each_append`,
  `test_spec_app_contract_6_snapshot_and_restore_reach_an_embedded_app`,
  `test_spec_app_contract_6_phase_2_methods_never_ok`,
  `test_spec_app_contract_6_auth_layer_wraps_an_embedders_router`,
  `test_spec_app_contract_7_query_cannot_write`.

---

## 7. Conformance mapping

Phase 1 can satisfy exactly these lines of `protocol.md §13`, quoted as written there, and
CI asserts them by name — `.github/scripts/conformance.sh`, since M13, runs the tests
below with `--exact` and fails when fewer ran than were named:

| Checklist item (§13 wording) | Milestone |
|---|---|
| Deleting `cache/` and all `snap/` directories loses no data (§3.1, §5) | M3 |
| Preserves unknown envelope and `d` fields byte-for-byte (§4.2) | M2 |
| Lamport counter is monotonic across restart and sync (§4.3) — restart half only | M2 |
| Rejects events > 24h in the future (§4.4) | M2 (log scan) + M3 (materialization) |
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

**R1 — the bundled engine build.** *(Written for DuckDB; ADR 0006 replaced it with SQLite,
whose amalgamation compiles in about a minute and closes this risk. Kept as written.)*
Compiling DuckDB from source is a multi-minute C++ build
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
*Decided in M12: `full_moon` 2.2.0 with `lua54` parses every reference app, every
`lib/` module, every compiled template and the whole corpus; the fallback was not built.
`deny.toml`'s `unmaintained = "workspace"` is what watches it from here.*

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

**R8 — Spec edits outrunning `skills/`.** §3 already changed nine spec sections, and
`docs/skills.md §7` makes an unreflected spec change an incomplete change. More will follow
once code meets contract. The CI drift check was scheduled for M13, which is too late to be
useful — add it in M0 as a warning and promote it to an error when the generator lands.
In M0 it cannot be a diff: `skills/*/reference/` holds placeholders and the generator is
a later milestone. What it can be, and is, is a recorded SHA-256 per `spec/` document
that warns when one changes. The generator was M13's and is M12's since M11 — eight
milestones of spec edits made the case that the check should exist while the linter, the
heaviest editor of `spec/` and `skills/`, is being written, not after — and it replaces
the hash manifest there. *Done in M12: `cargo xtask gen-skill-reference --check` is the
CI step, the hash manifest and `spec_drift.rs` are gone, and the generated files copy
whole spec sections, so an edit to a section a skill cites is drift by construction.*

---

## 9. PR sequence

| # | Branch | Depends on | Spec edits |
|---|---|---|---|
| 0 | `m0-workspace` | — | — |
| 1 | `m1-identity` | M0 | — |
| 2 | `m2-log` | M1 | — |
| 3 | `m3-materialize` | M2 | — |
| 4 | `m4-snapshots` | M3 | — |
| 5 | `m5-app-loader` | M3 | — |
| 6 | `m6-wire-http-shell` | M5 | — |
| 7 | `m7-lua-host` | M6 | — |
| 8 | `m8-lsp-hot-reload` | M7 | — |
| 9 | `m9-data-api` | M6, M8 | — |
| 10 | `m10-reference-apps` | M9 | as found |
| 11 | `m11-cli` | M10 | — |
| 12 | `m12-lint` | M11 | — |
| 12b | `phase1-hardening` | M12 | as found (§3 rows 90–98) |
| 12c | `phase1-hardening-2` | 12b | as found (§3 rows 99–102) |
| 13 | `m13-embedded-release` | M12 | roadmap: tick Phase 1 |
| 14 | `phase1-hardening-3` | 13 | as found (§3 rows 109–113) |
| 15 | `phase1-hardening-4` | 14 | as found (§3 rows 114–116) |

---

Copyright © 2026 Gabriel Mongefranco
