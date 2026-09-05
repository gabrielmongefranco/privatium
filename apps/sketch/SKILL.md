---
name: privatium-app-sketch
description: Context for extending the sketch reference app — a Tier 2 canvas with no server-side code and no SQL. Load alongside privatium-tier2-web when modifying this specific app.
---

# sketch

Tier 2. A shared canvas. No `app.lua`, no `views/`, no `schema.sql`.

## Storage model

The event log is used directly as a document store. Each stroke is one event:

```js
await pv.put('stroke', pv.ulid(), { points, color, width });
```

No schema means no validation — `d` is stored as given. This is deliberate and correct for
a drawing app.

## Conventions to preserve

- **Save on stroke end, not on pointer move.** Every append is a durable line in a log
  file — and, from Phase 3 of `docs/roadmap.md`, one that syncs to every device.
- `pv.subscribe` handles strokes from *other* windows today and, from Phase 3, from other
  devices, including ones that arrived via sync while this tab was closed. Do not assume
  local input is the only source.
- **The pointer is captured for the stroke.** Releasing it outside the canvas, a
  `pointercancel`, or a lost capture all end the stroke and save it; the stroke is taken
  off the in-progress slot before the append is awaited, so a stroke begun meanwhile is
  not cleared by the last one's handler.
- Boot reads the log in order through `pv.events({ tbl: 'stroke' })` — a `del` removes a
  stroke — and `pv.on('resync', load)` reads it again when the node rebuilt its cache.
- No CDN. Nothing is vendored today; if something is, it goes in `web/vendor/`.
- No inline `<script>` — the CSP forbids it, and `[permissions]` is deliberately all false.

## Accessibility gaps to fix, not replicate

The canvas carries an `aria-label`, the swatches announce the current colour through
`aria-pressed`, the viewport stays zoomable, and focus is navy on white. What is still
missing is a keyboard way to draw and any text description of what has been drawn. That
is a known deficiency in this reference app, not a pattern to copy. See
`privatium-accessibility`.

Two things to keep when touching the canvas: size it in `style.css`, and let `fit()` in
`app.js` match the backing store to `clientWidth`/`clientHeight` at `devicePixelRatio`.
Sizing from `innerWidth` draws past the viewport on every HiDPI display.

Run `privatium lint apps/sketch` before finishing.
