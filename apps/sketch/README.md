# sketch — the Tier 2 reference app

A shared canvas. Draw on the laptop, it shows up on the phone. Works offline; queued
strokes replay on reconnect.

It exists to prove one thing: **the framework does not impose an application model.**

## What is not here

| File | Present? |
|---|---|
| `views/` | **No.** No server-rendered HTML. |
| `app.lua` | **No.** No server-side code. |
| `schema.sql` | **No.** This app has no tables. |
| A build step | **No.** Plain ES modules; `app.js` is what ships. |

`app.toml` is ~20 lines. `web/` is three files: an HTML page, a stylesheet, and 80 lines of
JavaScript. That is the entire app.

## What the framework still gives it

Everything that matters, and none of it is in this folder:

- **Storage** — every stroke is an append-only event in `data/sketch/log/<device>.jsonl`
- **Sync** — strokes reach every paired device over LAN, iroh, or a synced folder
- **Auth** — the canvas is behind the node's pairing; the app implements nothing
- **Encryption** — session crypto is already applied by the time `fetch` returns
- **Discovery** — the phone finds the node with no URL typed
- **Backup** — copy `data/`, and every stroke you ever drew comes back
- **Offline** — writes queue in an outbox, replay on reconnect

## The event log as a document store

No `schema.sql` means no validation and no SQL — `d` is stored as-is:

```js
await pv.put('stroke', pv.ulid(), { points, color, width });
```

For a drawing app or a game this is frequently the right call. You still get replication,
snapshots, and a plain-text backup. Read your own drawing back with no Privatium installed:

```bash
jq -r '.d.color' data/sketch/log/*.jsonl | sort | uniq -c
grep -c '"op":"put"' data/sketch/log/*.jsonl
```

## Use a real framework if you want

The framework's own UI is HTMX and ships no client framework. That is a decision about
*the framework*. Your `web/` directory is yours — vendor React, Three.js, Chart.js, a WASM
blob, whatever the app needs. You pay for it in bytes on a phone, and that is your call.

This app uses vanilla JS because a canvas needs no framework, not because one was forbidden.

## Why this is not a Tier 1 app

A drawing canvas has no server-rendered form of itself. Tier 1 would mean shipping a
`<canvas>` and then writing all the JavaScript anyway, with an LSP template that does
nothing but wrap it. When the interface *is* the interaction, Tier 2 is the honest choice.

## Solo mode

```toml
# config.toml
[node]
mode = "solo"
app  = "sketch"
```

Now the binary *is* Sketch. Mounted at `/`, no launcher, its icon and title become the
node's. Indistinguishable from a purpose-built app.

---

Copyright © 2026 Gabriel Mongefranco
