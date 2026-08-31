<!--
Project:  Privatium™
File:     spec/data-api.md
Authors:  Gabriel Mongefranco (@gabrielmongefranco)
Created:  2026-08-28
Modified: 2026-08-31
Summary:  NORMATIVE. The HTTP data API that custom-UI (Tier 2) apps build against.
-->

# Data API — `api = 1`

The interface between an app's own front end and the framework's storage. This is what
makes Tier 2 possible: a replicated, syncing, backup-safe database reachable from any
JavaScript, WASM, or native client, with authentication already handled.

All endpoints are under `/a/<slug>/api/` and are scoped to that app. An app cannot read or
write another app's data through this API.

This namespace is reserved by the framework beneath an app's mount point
(`spec/protocol.md §9.1`); a Tier 2 app MUST NOT serve its own files at `web/api/`. It is
versioned by `app.api` in the manifest rather than by a path segment, so that an app
declares the contract it was written against.

All endpoints require a live session (`spec/protocol.md §8`). Cookies carry it; no token
handling is required in app code.

---

## 1. Read

### `GET /a/<slug>/api/q/<view>`

Run a named view from `views.sql`. Query-string parameters bind to `$name` placeholders.

```
GET /a/medtracker/api/q/v_upcoming?days=30
```

```json
{
  "view": "v_upcoming",
  "columns": [
    {"name":"drug","type":"VARCHAR"},
    {"name":"due_on","type":"DATE"},
    {"name":"copay_amount","type":"DECIMAL(18,2)"}
  ],
  "rows": [
    {"drug":"Example","due_on":"2026-09-04","copay_amount":"12.50"}
  ],
  "lam": 8830
}
```

- `columns` carries real DuckDB types so a client can format correctly.
- `DECIMAL` and `BIGINT` are JSON **strings**. See `spec/data-dictionary.md §2.1` — JSON
  numbers are doubles in most parsers and would silently corrupt money.
- `lam` is the Lamport high-water mark the result reflects. Pass it to `/stream` to resume
  without a gap.
- Pagination: `?limit=` and `?offset=`. Default limit 1000, maximum 10000.

### `POST /a/<slug>/api/sql`

Ad-hoc read-only SQL. **Requires `permissions.sql = true` in `app.toml`.**

```json
{ "sql": "SELECT drug, sum(copay_amount) AS total FROM fill WHERE filled_on > ? GROUP BY 1",
  "params": ["2026-01-01"] }
```

- Statement MUST be a single `SELECT` or `WITH ... SELECT`. Anything else is rejected.
- Runs on the sandboxed connection (`spec/app-contract.md §7`). File functions are
  unavailable regardless of this permission.
- Parameters are bound, never interpolated. Implementations MUST reject a request
  containing `?` placeholders with a mismatched `params` length rather than substituting.
- Rate limited. Default 20 requests per second per session.

### `GET /a/<slug>/api/row/<tbl>/<id>`

Single row by ULID. Returns `404` if absent or tombstoned.

### `GET /a/<slug>/api/events`

Raw event access, for apps that use the log as a document store rather than as tables.

```
GET /a/medtracker/api/events?tbl=state&id=game
GET /a/medtracker/api/events?after=8800&limit=500
```

Returns raw event lines as NDJSON, byte-identical to what is on disk.

---

## 2. Write

### `POST /a/<slug>/api/events`

Append a batch. Atomic: all lines or none.

```json
{
  "events": [
    {"op":"put","tbl":"stroke","id":"01J9YQ...","d":{"points":[[0,0],[4,9]],"color":"#00274C"}},
    {"op":"del","tbl":"stroke","id":"01J9YP..."}
  ]
}
```

The client supplies only `op`, `tbl`, `id`, and `d`. The framework stamps `seq`, `lam`,
`ts`, `dev`, and `app` — a client MUST NOT set these and the server MUST reject a request
that does.

Response:

```json
{ "appended": 2, "lam": 8832, "ids": ["01J9YQ...", "01J9YP..."] }
```

Constraints:

- Maximum 1000 events per batch, maximum 4 MB per request.
- `id` MUST be a valid ULID. Mint one client-side with `pv.ulid()` or server-side by
  omitting `id` (the server mints and returns it).
- If the app has a `schema.sql`, `NOT NULL` and `CHECK` constraints are validated before
  the append and a violation rejects the whole batch with the offending index.
- If the app has no `schema.sql`, `d` is stored as-is with no validation.

### Nothing else

There is no framework-defined action endpoint. An earlier draft specified
`POST /a/<slug>/api/x/<action>` for invoking named server-side actions; that belonged to the
declarative tier and was removed with it.

An app has exactly one tier (`app.toml` `tier`), so there is no Tier 1 handler for a Tier 2
front end to call. A Tier 2 app that wants server-side logic writes it against
`spec/app-contract.md §6` (Tier 3) or performs it client-side.

---

## 3. Live updates

### `GET /a/<slug>/api/stream`

Server-Sent Events. Emits every event appended to this app **from any device**, including
events arriving via sync from another node.

```
GET /a/medtracker/api/stream?after=8830
```

```
event: append
data: {"lam":8831,"dev":"b3nn8t2q","op":"put","tbl":"fill","id":"01J9...","d":{...}}

event: resync
data: {"reason":"rematerialized","lam":8900}
```

| Event | Meaning |
|---|---|
| `append` | One new event. `after=` guarantees no gap on reconnect. |
| `resync` | State changed underneath you (schema change, restore, bulk sync). Re-query. |
| `ping` | Keep-alive, every 30 seconds. |

SSE rather than WebSocket because it reconnects automatically, survives proxies, and needs
no framing. Apps needing bidirectional streaming may open `/ws` and speak the session
protocol directly.

**A host MUST serve this endpoint as SSE.** A host MAY additionally offer a long-poll
fallback, and a client MAY negotiate it with `Accept: application/json` plus `after=`,
receiving the same event objects in a JSON array and reissuing the request on each response.
The fallback exists because custom-scheme streaming inside a platform webview — WKWebView in
particular — is unproven; it is not an invitation to skip SSE. `pv.js` selects between them
and apps see no difference (`§5`).

**Note:** a quick Cloudflare tunnel does not pass SSE. This does not affect LAN, Tailscale,
Let's Encrypt, onion, or native transports.

---

## 4. Metadata

### `GET /a/<slug>/api/schema`

Tables, columns, types, and available views and actions. Lets a generic client render an
app it has never seen.

### `GET /a/<slug>/api/node`

Node ID, device ID, display name, `solo` flag, sync peer count, restore tier in use.
No application data.

---

## 5. The `pv.js` helper

Served at `/static/pv.js`. Roughly 4 KB, no dependencies, no framework. **Optional** — every
endpoint is plain HTTP and `fetch` works fine.

```js
import { pv } from '/static/pv.js';

const rows  = await pv.query('v_upcoming', { days: 30 });
const rows2 = await pv.sql('SELECT * FROM fill WHERE drug = ?', ['Example']);
const row   = await pv.get('state', 'game');

await pv.append([{ op:'put', tbl:'state', id:'game', d: state }]);
await pv.put('state', 'game', state);          // sugar for the above
await pv.del('stroke', id);
const result = await pv.action('learn', { animal:'wombat' });

const stop = pv.subscribe(ev => { if (ev.tbl === 'stroke') redraw(ev); });

pv.ulid();        // client-side ULID
pv.node();        // cached /api/node
```

`pv.query` and `pv.sql` return plain arrays of objects. `DECIMAL` columns arrive as
strings; the helper does **not** convert them to numbers, because that is exactly the bug
this design exists to prevent. Use a decimal library or integer cents in your own code.

---

## 6. Offline

The API is local. In a native shell or an installed PWA it is served by the node process on
the same device, so it works with no network at all.

When the node is remote and unreachable, `pv.append` queues to an outbox and replays on
reconnect; `pv.query` throws `BwOffline` and the app decides what to show. The helper
exposes `pv.online` and a `pv.on('online' | 'offline')` event.

**Replay is idempotent and needs no bookkeeping.** A queued write carries its ULID, so
resending one that may already have landed converges to the same row under
`spec/protocol.md §4.5`. Apps MUST NOT implement their own deduplication, transaction
identifiers, or acknowledgement protocol — all three indicate a misreading of the merge
rule, and all three can introduce the divergence they were meant to prevent.

Endpoint selection and failover are handled by the client runtime
(`spec/protocol.md §10.4`), not by the app. In a browser there is exactly one endpoint — the
page's own origin — because mixed content forbids an HTTPS page from fetching a plain-HTTP
LAN address. Apps MUST NOT construct absolute URLs to other endpoints; use `pv.url()`.

An app that must work fully offline against a remote node should keep its own read cache.
The framework does not impose one, because what to cache is an application decision.

---

## 7. Rate limits and quotas

| Limit | Default | Setting |
|---|---|---|
| Ad-hoc SQL | 20/s per session | `api.sql_rate` |
| Append batch size | 1000 events | `api.max_batch` |
| Request body | 4 MB | `api.max_body` |
| Query result rows | 10000 | `api.max_rows` |
| SSE connections | 8 per device | `api.max_streams` |

These protect the node from a buggy app, not from a hostile one. A hostile app you
installed has already won at the SQL level.

---

Copyright © 2026 Gabriel Mongefranco
