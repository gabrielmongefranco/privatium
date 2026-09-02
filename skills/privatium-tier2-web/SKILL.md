---
name: privatium-tier2-web
description: Write Tier 2 Privatium apps with your own HTML, CSS, JavaScript, or WASM in a web/ directory, using the pv.js data API for storage and sync. Covers the API surface, CSP permissions, offline behaviour, and library choice. Load for games, canvas, charts, animation, or any app with its own interaction design.
---

# Privatium Tier 2 — Custom Web UI

You write the front end; the framework serves it and handles storage, sync, auth, and
encryption. `web/index.html` is served at the app's mount point. Nothing is injected.

```
apps/<slug>/
├── app.toml          tier = "web"
├── web/
│   ├── index.html
│   ├── app.js
│   ├── style.css
│   └── vendor/       whatever you vendored
└── schema.sql        OPTIONAL
```

## The API

```js
import { pv } from '/static/pv.js';

const rows = await pv.query('v_upcoming', { days: 30 });
const rows2 = await pv.sql('SELECT * FROM fill WHERE drug = ?', ['Example']); // needs permission
const row  = await pv.get('state', 'game');

await pv.put('state', 'game', myState);
await pv.append([{ op:'put', tbl:'stroke', id: pv.ulid(), d:{ points } }]);
await pv.del('stroke', id);

const stop = pv.subscribe(ev => { if (ev.tbl === 'stroke') redraw(ev); });
pv.on('offline', () => …);  pv.on('online', () => …);

pv.ulid();  pv.url('/path');  pv.node();
```

`pv.js` is optional; every endpoint is plain HTTP under `/a/<slug>/api/`.

## MUST

- Use `pv.url()` for internal links — hardcoded `/a/<slug>/` breaks solo mode
- Keep `DECIMAL` values as strings; `pv.query` deliberately does not convert them
- Put scripts in external files — the default CSP is `script-src 'self'`, no inline
- Declare every non-default permission in `app.toml` and be able to justify it
- Handle `pv.subscribe` events from *other* devices, not just local input
- Vendor libraries into `web/vendor/`; never load from a CDN

## MUST NOT

- Set `seq`, `lam`, `ts`, `dev`, or `app` on an event — the server rejects it
- Use `localStorage` for anything that should survive a device — that is what the log is for
- Assume you are online; `pv.query` throws `PvOffline`
- Implement your own outbox deduplication, transaction IDs, or acknowledgement protocol.
  ULIDs already make replay idempotent — adding these can create the divergence they were
  meant to prevent.
- Construct absolute URLs to other endpoints. A browser client has exactly one origin.
- Request `permissions.remote` unless the app genuinely must call out. It is the one thing
  this project exists to avoid, and the installer says so to the owner.

## Storage without SQL

Omit `schema.sql` and the log is a document store:

```js
await pv.put('state', 'game', entireGameState);
```

No validation, no SQL — but full replication, snapshots, and plain-text backup. For a game
or a drawing app this is usually right.

## Permissions

```toml
[permissions]
inline_script          = false   # prefer an external file
wasm                   = false   # 'wasm-unsafe-eval'
eval                   = false   # 'unsafe-eval'
remote                 = []      # extra script/img/connect origins
sql                    = false   # ad-hoc read-only SQL via pv.sql()
cross_origin_isolated  = false   # SOLO MODE ONLY — see privatium-games
```

## Libraries

Zero build, vendored: Alpine.js, Datastar, VanJS, Preact+htm, Lit, PixiJS, Chart.js, uPlot,
D3, wasmoon.

Build step you run and commit: Svelte, SolidJS, Vue SFC, React.

Never: Appsmith, Budibase, Saltcorn, NocoBase, Grist, Retool — those are platforms with
their own backends, not libraries. Webix is GPL/commercial dual-licensed; avoid.

Full matrix with reasoning: `docs/frameworks.md`.

## Verify

```bash
privatium lint apps/<slug>
```

Spec: `spec/data-api.md`.
