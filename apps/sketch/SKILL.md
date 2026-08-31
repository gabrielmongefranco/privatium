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

- **Save on stroke end, not on pointer move.** Every append is a durable line in a log file
  that syncs to every device.
- `pv.subscribe` handles strokes from *other* devices, including ones that arrived via sync
  while this tab was closed. Do not assume local input is the only source.
- No CDN. Nothing is vendored today; if something is, it goes in `web/vendor/`.
- No inline `<script>` — the CSP forbids it, and `[permissions]` is deliberately all false.

## Accessibility gaps to fix, not replicate

The canvas currently has no keyboard alternative and no text description of its content.
That is a known deficiency in this reference app, not a pattern to copy. See
`privatium-accessibility`.

Run `privatium lint apps/sketch` before finishing.
