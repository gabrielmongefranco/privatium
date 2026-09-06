# sketch — the Tier 2 reference app

A shared canvas. Draw in one browser window and it shows up in another. A page already
open can queue strokes while disconnected and replay them on reconnect. A plain-HTTP
LAN page cannot load anew while the node is offline. A phone on the same network pairs
by scanning the QR code the node prints and tapping four emoji, then draws on the same
canvas over the encrypted channel. Draw with a pointer, or from the keyboard: focus the
canvas, move the pen with the arrow keys, put it down with Space.

It exists to prove one thing: **the framework does not impose an application model.**

## What is not here

| File | Present? |
|---|---|
| `views/` | **No.** No server-rendered HTML. |
| `app.lua` | **No.** No server-side code. |
| `schema.sql` | **No.** This app has no tables. |
| A build step | **No.** Plain ES modules; `app.js` is what ships. |

`web/` contains an HTML page, a stylesheet and three small JavaScript modules.
Together with `app.toml`, that is the entire app.

The toolbar wraps on small screens and labels every color. Open **Help** for
keyboard instructions. Help and status text stay readable when zoomed; see
[Sample app design](../../docs/sample-app-design.md) for the layout and its limits.

## What the framework still gives it

The framework provides:

- **Storage** — every stroke is an append-only event in `data/sketch/log/<device>.jsonl`
- **Backup** — copy `data/`, and every stroke you ever drew comes back
- **Offline** — writes queue in an outbox, replay on reconnect
- **Live updates** — a stroke drawn in one window reaches every other open window
- **Authentication and encryption** — `pv.js` uses the paired browser's encrypted
  channel on the LAN; no credential code belongs in this app

What arrives with the later phases of `docs/roadmap.md`, with nothing to change here:

- **Sync** (Phase 3) — strokes reach every paired device over LAN, iroh, or a synced folder

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
