<!--
Project:  Privatium™
File:     spec/app-contract.md
Authors:  Gabriel Mongefranco (@gabrielmongefranco)
Created:  2026-08-28
Modified: 2026-08-31
Summary:  NORMATIVE. What an app is, the three tiers of app, and the three
          deployment modes. The declarative tier is one option, not the model.
-->

# App Contract — `api = 1`

**An app is a folder containing an `app.toml`. Everything else is optional.**

The framework's job is storage, sync, discovery, pairing, encryption, and reachability.
It is not an application model. How you build your application is your business; the three
tiers below exist so you can take as much or as little help as you want.

---

## 1. The three tiers

Tiers are distinguished by **language**, not by how much freedom you give up. None of them
has a ceiling. Pick per app; a single node hosts all three at once.

| Tier | Language | Framework renders | Fits |
|---|---|---|---|
| **1 — Lua** | Lua 5.4 + LSP templates | Your templates, server-side | Records, lists, forms, reports, trackers |
| **2 — Web** | Your own HTML / CSS / JS / WASM | Nothing — it serves your files | Games, canvas, charts, animation |
| **3 — Rust** | Rust against `privatium-core` | Nothing — you own `main()` | Hardware, scheduled jobs, non-HTTP protocols |

**There is no declarative tier.** An earlier draft defined an app as `schema.sql` plus
`views.sql` plus `forms.toml`. It was removed: it had a hard expressiveness ceiling and it
imposed an application model the framework has no business imposing. What survives is a
**scaffold generator** (§4.6) that emits Lua and templates you then edit — a starting point
you escape from, not a runtime you are trapped in.

**Tier 1 is the default.** No build step, hot reload, no JavaScript required. Move to
Tier 2 only when the interface genuinely cannot be server-rendered HTML.

What every tier gets regardless:

- Append-only event storage that survives file sync (`spec/protocol.md §4`)
- Multi-device sync with no server (`§10`)
- Discovery, pairing, session encryption, device revocation (`§6–8`)
- LAN, remote, onion, and native-shell reachability
- Snapshots, three-tier restore, plain-text backup

That list is the product. The rest is optional scaffolding.

---

## 2. The three deployment modes

Orthogonal to tier. Set in `config.toml`.

### 2.1 Host mode (default)

One binary, many unrelated apps mounted at `/a/<slug>/`, with a launcher at `/`. One
pairing, one backup folder, one discovery record.

### 2.2 Solo mode

```toml
[node]
mode = "solo"
app  = "medtracker"
```

The same binary, one app, mounted at `/`. No launcher, no `/a/<slug>/` prefix. The app's
title and icon become the node's. Discovery advertises it directly.

To an end user this is indistinguishable from a purpose-built application. Use it when you
are shipping *your app*, not *a node that happens to run it*.

### 2.3 Embedded mode

```toml
[dependencies]
privatium-core = "1"
```

Your `main()`, your HTTP stack (or none), your routes. `privatium-core` gives you the log,
the materializer, sync, discovery, and pairing as a library. The framework has no opinion
about anything else.

```rust
let node = privatium::Node::open(&data_dir)?;
node.serve_discovery()?;          // mDNS, UDP, pairing
node.start_sync()?;               // iroh + LAN peers

// your own writes
node.append("myapp", Event::put("score", &id, json!({"points": 42})))?;

// your own reads — the materialized DuckDB connection, sandboxed
let rows = node.query("myapp", "SELECT * FROM score ORDER BY points DESC")?;

// your own server
axum::serve(listener, my_router.layer(node.auth_layer())).await?;
```

This is the shape to use when Privatium is a dependency of your app rather than the other
way round. It is a first-class mode, not an escape hatch.

---

## 3. `app.toml` — the only required file

```toml
[app]
slug        = "myapp"          # REQUIRED  ^[a-z][a-z0-9-]{1,30}$
title       = "My App"         # REQUIRED  ≤ 40 chars
version     = "1.0.0"          # REQUIRED  semver
api         = 1                # REQUIRED  framework API targeted
tier        = "lua"            # REQUIRED  "lua" | "web" | "rust"
description = "..."
icon        = "diagram-3"      # Bootstrap Icons filename; see docs/icons.md
authors     = ["..."]
license     = "GPL-3.0-or-later"

[nav]
order     = 10
advertise = true               # advertise a DNS-SD subtype
```

Everything below `[app]` and `[nav]` depends on the tier. A Tier 2 app's manifest can be
this and nothing more.

### 3.1 Validation

A node MUST refuse to load an app and MUST record `app.load_failed` when the slug is
reserved or malformed, when it collides with an installed app, or when `api` exceeds what
the framework implements.

Tier-specific validation applies only to that tier. Refusal is per-app and loud; one broken
app MUST NOT prevent the node from starting.

---

## 4. Tier 1 — Lua

```
apps/<slug>/
├── app.toml
├── app.lua          REQUIRED  routes and handlers
├── lib/             your own modules
├── views/           LSP templates (.lsp)
├── static/          css, images, vendored js
├── schema.sql       OPTIONAL  tables, if you want SQL
└── migrations/      OPTIONAL
```

Full API in `spec/lua-api.md`. Summary follows.

### 4.1 Handlers

```lua
local pv = require 'privatium'

pv.get('/', function(req)
  return pv.render('index', { fills = pv.query('SELECT * FROM fill ORDER BY filled_on DESC') })
end)

pv.post('/fill', function(req)
  pv.append('fill', { drug = req.form.drug, copay_amount = req.form.copay_amount })
  return pv.redirect(url('/'))
end)
```

Handlers return `pv.render`, `pv.redirect`, `pv.json`, `pv.text`, a string, or `nil`.

### 4.2 LSP templates

`views/*.lsp` is HTML with embedded Lua, compiled to a cached Lua chunk and invalidated on
file mtime. Save, refresh, done — no build, no restart.

| Tag | Meaning |
|---|---|
| `<? ... ?>` | Execute, emit nothing |
| `<?= expr ?>` | Emit, **HTML-escaped** |
| `<?raw expr ?>` | Emit unescaped. Flagged by the linter every time. |
| `<?-- ... --?>` | Comment |

Escaping is the default and there is no flag to disable it.

### 4.3 Reads and writes

Reads are SQL: `pv.query(sql, params)`. Writes are appends: `pv.append(tbl[, id], data)`,
`pv.delete(tbl, id)`, and `pv.batch(fn)` for atomic multi-event writes.

Implementations MUST reject string-concatenated SQL at lint time. `DECIMAL` and `BIGINT`
are strings; `pv.dec()` provides exact arithmetic.

### 4.4 Where logic lives is the author's choice

Handlers, `lib/` modules, and SQL views are all legitimate homes for application logic. The
framework has no preference and imposes no split. Views suit set-shaped questions; Lua suits
branching, formatting, and anything awkward to express in SQL. Most apps use both.

The one thing worth knowing: a native mobile client that embeds the core library can hold a
full replica and evaluate SQL locally, but cannot evaluate Lua handlers. So an app whose
logic is in views has an easier path to full mobile offline than one whose logic is in Lua.
That is a consequence to be aware of, **not a reason to contort an app into SQL.** Most apps
should ignore it.

### 4.5 `schema.sql` is optional

Include it for typed tables and SQL queries; omit it to use the event log as a document
store. Every table needs `id VARCHAR PRIMARY KEY` holding a ULID. Use `DECIMAL(18,2)` for
money and `DATE` for dates.

Changing `schema.sql` rematerializes from the logs. Safe at any time, loses nothing; new
columns are NULL for old events.

`migrations/` is **reserved and not implemented in `pv/1`** (`spec/data-dictionary.md §3.11`).
It would be needed only when the *meaning* of stored data changes — a unit conversion, a
re-encoding — and no such case exists yet. A migration will transform events at replay time;
it will never mutate a log.

### 4.6 Sandbox

`io`, `os.execute`, `os.getenv`, `debug`, `load`, `dofile`, and `package.loadlib` are
removed. `require` is confined to the app's `lib/`. Instruction, memory, and wall-clock
limits are enforced per request. Lua states are pooled, so globals are per-VM and do not
persist — shared state MUST go through the event log. See `spec/lua-api.md §5`.

### 4.7 Scaffolding

```bash
privatium new medtracker                    # empty Lua app
privatium new medtracker --scaffold fill    # CRUD for a table in schema.sql
```

Emits `app.lua` and `views/*.lsp` as ordinary source files. The generator has no runtime
presence; delete or rewrite anything it produced. It exists so nobody hand-writes a fifth
list-and-form screen, not to define what an app is.

### 4.8 Tier 1 on mobile

Tier 1 renders on the node. A mobile client fetches **HTML**, never Lua — so a Tier 1 app
works on every platform including iOS, is delivered dynamically, and updates the instant the
node's files change. No rebuild, no store submission, no per-user compilation.

The limit is offline. A client that cannot reach the node has cached pages and an outbox,
but **cannot render a view it has not already seen**, because the code that produces it lives
on the node.

| | Tier 1 (Lua) | Tier 2 (Web) |
|---|---|---|
| Runs on iOS and Android | ✔ | ✔ |
| Delivered dynamically, updates instantly | ✔ HTML | ✔ HTML/JS/WASM |
| Offline — cached reads | ✔ | ✔ |
| Offline — queued writes | ✔ | ✔ |
| **Offline — render a view not yet visited** | ✘ | ✔ from the local replica |
| **Offline — compute over the full dataset** | ✘ | ✔ |

Implementations MUST NOT attempt to close this by shipping the Lua interpreter a copy of a
downloaded `app.lua`. Executing downloaded native code is prohibited on at least one target
platform, and the exemption that makes dynamic delivery legal covers scripts run by the
platform web view — which is what Tier 2 already uses.

**If full offline capability on mobile is a requirement, that is a reason to choose Tier 2.**
It is not a defect in Tier 1; it is the cost of rendering on the node, which is also what buys
Tier 1 its single implementation, its absence of a build step, and its hot reload.

Two mitigations an author may use without leaving Tier 1:

- **Warm the cache.** Have the client prefetch the app's main views while connected, so the
  common screens are available offline.
- **Put set-shaped logic in SQL views.** A replica client materializes locally and can
  evaluate SQL, which every platform treats as data rather than executable code. This is an
  option, not a guideline — do not contort an app into SQL to obtain it (§4.4).

## 5. Tier 2 — custom web UI

You write the front end. The framework serves your files and exposes the data.

```
apps/<slug>/
├── app.toml            tier = "web"
├── web/                served at /a/<slug>/
│   ├── index.html      entry point
│   ├── app.js
│   ├── style.css
│   └── vendor/         whatever you vendored
└── schema.sql          OPTIONAL — typed tables, and `CREATE VIEW` for `/api/q/<view>`
```

`web/index.html` is served at the app's mount point. Everything under `web/` is served
as-is. There is no build step imposed and no framework injected — if you want a build step,
run it yourself and commit the output.

### 5.1 What you can use

**Anything.** Canvas, WebGL, WebGPU, Web Audio, WASM, SVG, Chart.js, uPlot, D3, Three.js,
Svelte, Preact, or 300 lines of vanilla JS.

The framework's own UI is HTMX and ships no client framework — that is a decision about the
*framework*, not a rule imposed on *your app*. If you want React inside your app folder,
vendor it. You will pay for it in bytes on a phone, which is your call to make.

Your app is served from the same origin as the framework, so it inherits the session and
the encryption. You do not implement auth.

### 5.2 The data API

Documented normatively in `spec/data-api.md`. Summary:

```js
// read a named view
const rows = await pv.query('v_upcoming', { days: 30 });

// read ad-hoc SQL (requires the sql permission)
const rows = await pv.sql('SELECT * FROM fill WHERE filled_on > ?', ['2026-01-01']);

// write events
await pv.append([
  { op: 'put', tbl: 'stroke', id: pv.ulid(), d: { points, color, width } }
]);

// live updates from any device
pv.subscribe(ev => redraw(ev));
```

`pv` is a ~4KB script served by the framework at `/static/pv.js`. It is optional — the
endpoints are plain HTTP and you can `fetch` them yourself.

### 5.3 Storage without SQL

An app that does not want tables can omit `schema.sql` entirely and use the event log as a
document store:

```js
await pv.append([{ op:'put', tbl:'state', id:'game', d: myEntireGameState }]);
const { d } = await pv.get('state', 'game');
```

You still get sync, replication, snapshots, and plain-text backup. You just do not get SQL
queries over it. For a drawing app or a game this is frequently the right choice.

### 5.4 Content Security Policy

Each app is served with its own CSP. By default `script-src 'self'` scoped to the app's
own path — external `.js` files work, inline `<script>` does not.

**Inline event handler attributes are script too.** `onclick`, `onsubmit`, and their
relatives are blocked by the same default. This is the most common way an otherwise correct
app fails silently: nothing errors, the handler simply never runs.

**Reactive micro-frameworks usually need a CSP-specific build.** Alpine is the common case:
its standard build compiles attribute expressions with the `Function` constructor and
therefore requires `'unsafe-eval'`, while `@alpinejs/csp` drops inline expressions in favour
of components registered with `Alpine.data()` and needs no permission at all. Reach for the
CSP build rather than the `eval` permission — granting `eval` to shorten some attributes
hands any injected string a JavaScript engine. `apps/animals` is a worked example.

Some things need more. Declare it and take the warning:

```toml
[permissions]
inline_script = false   # allows 'unsafe-inline' — avoid; use an external file
wasm          = false   # allows 'wasm-unsafe-eval'
eval          = false   # allows 'unsafe-eval'; some older WASM loaders need it
remote        = []      # additional origins for script-src/img-src/connect-src
sql           = false   # allow ad-hoc read-only SQL via pv.sql()
```

Every non-default permission is shown to the owner at install time in plain language.
`remote` in particular means "this app phones out," which is the one thing this project
exists to avoid; the installer says so.

---

## 6. Tier 3 — Rust

Your binary, `privatium-core` as a dependency. See §2.3.

You may also register a Tier 3 app *inside* a host-mode node by implementing the plugin
trait and compiling it in — but this means a custom build of the node binary, which is why
Tier 2 exists. Reach for Tier 3 when you need server-side work the HTTP surface cannot
express: a serial port, a scheduled job, a filesystem watcher, a non-HTTP protocol.

`privatium-core` API surface, stable across `api = 1`:

| Area | What you get |
|---|---|
| `Node::open` / `close` | Data root, identity, materialization |
| `append` / `append_batch` | Event writes with automatic `seq`/`lam`/`ts` |
| `query` / `subscribe` | Sandboxed DuckDB reads; event stream |
| `serve_discovery` / `pair` | mDNS, UDP, PAKE pairing, device registry |
| `start_sync` / `sync_now` | iroh + LAN peers |
| `auth_layer` | Tower middleware enforcing session and grants |
| `snapshot` / `restore` | Manual snapshot and three-tier restore |

---

## 7. The SQL sandbox (Tiers 1 and 2)

The connection app SQL runs on MUST be configured with at minimum:

```sql
SET enable_external_access = false;
SET autoinstall_known_extensions = false;
SET autoload_known_extensions = false;
SET lock_configuration = true;        -- must be last
```

This is not optional hardening — DuckDB with external access enabled can read
`identity/node.key`.

**The boundary is in time, not in two connection handles.** These four settings are
`GLOBAL_ONLY` in DuckDB: they belong to the database instance, not to a connection. A
`.duckdb` file is also locked exclusively, so a second instance cannot open one app's cache
alongside the first. There is therefore no arrangement in which a privileged connection and
a sandboxed connection coexist over the same cache, and an implementation that appears to
have both has really sandboxed neither.

What an implementation MUST do instead:

1. Open the cache **privileged** — external access on, autoload and autoinstall off, since
   materializing reads the log files through `read_json()`.
2. Materialize.
3. Apply the four settings above, `lock_configuration` last. From this point **no**
   connection on the instance can perform file I/O, the framework's own included.
4. Serve app SQL from that instance.

Rematerializing, and writing a snapshot (`spec/protocol.md §5`), need step 1 again and
therefore a fresh instance. Applying a newly appended event does not: it is `DELETE` plus
`INSERT` over values the framework already holds, and touches no file.

Tier 3 runs in your own process and is not sandboxed. That is the trade: full power, full
responsibility.

---

## 8. Lifecycle

```
folder appears in apps/
  → parse app.toml                       (fail → app.load_failed, stop)
  → validate slug, api, tier
  → tier 1: load app.lua in a sandboxed VM, register routes, compile views/, hash
  → tier 2: verify web/index.html exists; compute CSP from [permissions]
  → tier 3: (linked at compile time; index entry only)
  → upsert sys_app                       (event)
  → materialize from data/<slug>/        (if the app has tables)
  → mount, advertise subtype
```

---

## 9. Publishing

An app is a folder, so distribution is a zip or a git repository. There is no registry in
`pv/1`, deliberately: a registry implies curation, curation implies a trust signal, and a
false trust signal is worse than none.

**Warn the owner at install.** A Tier 1 app's Lua runs on their node against their data. A
Tier 2 app's JavaScript runs in their browser on their session's origin. Both are
sandboxed from the filesystem, but treat installing a third-party app the way you would
treat running a script someone emailed you — because that is what it is.

Include `sample/seed.jsonl` (synthetic events only) so an owner can see the app populated.

---

Copyright © 2026 Gabriel Mongefranco
