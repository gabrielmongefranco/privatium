---
name: privatium-app-sketch
description: Context for extending the sketch reference app — a Tier 2 canvas with no server-side code and no SQL. Load alongside privatium-tier2-web when modifying this specific app.
---

# sketch

Tier 2. A shared canvas. No `app.lua`, no `views/`, no `schema.sql`.

## Storage model

The event log is used directly as a document store. Each mark is one event:

```js
await pv.put('stroke', pv.ulid(), { points, color, width });
```

No schema means no validation — `d` is stored as given. This is deliberate and correct for
a drawing app.

A freehand stroke keeps `{ points, color, width }`. Everything else is additive, so older
logs replay unchanged:

| Mark | `d` |
|---|---|
| Freehand, eraser | `{ points:[[x,y]] or [[x,y,w]], color, width }` |
| Line, rectangle, ellipse | `{ kind, a:{x,y}, b:{x,y}, color, width, dash? }` |
| Text | `{ kind:'text', x, y, text, color, width }` |
| Flood fill | `{ kind:'fill', x, y, color, anchor?, anchorAt? }` |

`blend: 'multiply'` on any inked mark is translucent ink, and `layer` is the painting-order
key history.js preserves across restoration. A width per point appears only when a stylus
varied it; a mouse or a finger leaves the two-number form alone.

## Conventions to preserve

- **Save on mark end, not on pointer move.** Every append is a durable line in a log file
  that syncs to every device.
- `pv.subscribe` handles marks from *other* windows and, over sync, from other devices,
  including ones that arrived while this tab was closed. Do not assume local input is the
  only source. A Lamport cursor is not a gap-free resume: re-read on `resync` **and** on
  reconnect.
- **The pointer is captured for the mark.** Releasing it outside the sheet, a
  `pointercancel`, or a lost capture all end the mark and save it; the mark is taken off
  the in-progress slot before the append is awaited, so one begun meanwhile is not cleared
  by the last one's handler.
- Boot reads the log in order through `pv.events({ tbl: 'stroke' })` — a `del` removes a
  mark — and `pv.on('resync', load)` reads it again when the node rebuilt its cache.
- A refused write reaches `pv.on('rejected')`. Say so in `#status`: it is the person's work.
- No CDN, no vendored library, no build step. `web/` is an HTML page, a stylesheet and
  seven ES modules: `app.js` (behaviour), `sheet.js` (coordinates, zoom and the device
  ratio), `paint.js` (drawing one mark, and the flood), `strokes.js` (geometry and hit
  testing), `clip.js` (clipboard and SVG), `tools.js` (the tool, size, ink and contrast
  tables), `history.js` (undo and redo).
- No inline `<script>` and no `style` attribute — the CSP has neither `script-src`
  `'unsafe-inline'` nor a `style-src`, and `[permissions]` is deliberately all false. Every
  colour lives in `style.css`; the values that follow the person's own choices are set as
  custom properties from `app.js`.

## The sheet

The sheet is a **fixed 1600 × 1200 coordinate space**. That is not the canvas's pixel size:
the element is sized in CSS to fit or zoom, its backing store is that box times
`devicePixelRatio` (never below 1600 × 1200), and the ratio rides in
`ctx.setTransform(k, 0, 0, k, 0, 0)` so every drawing routine speaks sheet coordinates.
Sizing the sheet from the window is the bug that once put a desktop drawing in the corner
of a phone; sizing the backing store from `innerWidth` draws past the viewport on every
HiDPI display. `sheet.js` owns all of it — nothing else needs to know the ratio, except
the two operations that are unavoidably in pixels, the flood and the eyedropper.

Zoom is per-device view state in `localStorage` under `sketch.zoom`; it must never reach
the log. Buttons step 5 points at a time, the percentage is editable, and 0 fits.

## Tools

Ten: Select, Brush, Eraser, Eyedropper, Fill, Line, Rectangle, Ellipse, Text, Pan.

- **Select** moves whole marks, never a rectangle of pixels — a pixel marquee cannot be
  expressed as edits to the log, and the canvas is shared. Stroke mode steps through
  overlapping marks on a repeat click; Rectangle mode takes every mark its box touches.
  A move is a tombstone and a fresh put per mark, carrying `layer`, written as one batch
  so one undo returns it.
- **Fill** is a seed point replayed in log order, not a raster snapshot. A fill that lands
  inside a rectangle or ellipse records that shape's id and its origin at the time, and
  resolves its seed against the shape's current origin, so it travels with it. **A fill
  whose anchor is not in the log is not drawn** — under sync it can arrive before its
  shape, and flooding from a stale seed would cover the sheet. Copying a shape brings its
  anchored fills; restoring one re-points them (`history.js` returns the id map).
- **Eyedropper** is momentary: it writes into the selected colour and hands back to the
  tool it interrupted, and it is disabled where there is no colour to pick into.
- **Translucent ink** is a property of the mark, not a mode, so replay never depends on
  the toggle's position. The eraser never blends, and a blended mark is composited once
  from an offscreen buffer so it does not darken against its own overlaps.
- **Smoothing** happens on capture and on render: coalesced samples, a distance-adaptive
  filter, a 1.2 px minimum spacing, and curves through sample midpoints rather than a run
  of straight lines.
- Copy, cut and paste move marks within the sheet; copying also writes stamped SVG to the
  system clipboard, and a foreign SVG paste is read through a deliberately small subset
  (straight geometry only) which reports what it left out.

## Accessibility

A drawing is not a pointer-only or a sight-only thing here, and a change must keep it so
(`privatium-accessibility`):

- **The keyboard draws.** The sheet is focusable (`tabindex="0"`). While it has focus a
  dashed crosshair marks the pen; the arrow keys move it, Shift moves it farther, Space or
  Enter puts it down and lifts it, Escape discards the mark in progress. A mark lifted
  from the keyboard is saved exactly as a pointer's is. Every tool has a letter, Delete
  removes a selection, and the arrow keys move a selection or pan the view when those
  tools are active.
- **What happens is said.** `#status` (`role="status"`) announces tool and colour changes,
  pen down, saved, discarded, picked colours, zoom, selection counts, refusals and the
  offline state. `#summary` (`aria-live="polite"`) says how many marks in how many colours
  on a 1600 by 1200 sheet.
- **What is drawn is described.** The sheet is `aria-describedby` the key hint and the
  summary.
- Every tool shows an icon **and** a visible text label in the rail; the top bar's
  duplicates are icon-only with accessible names, and their labelled instance is the rail.
  Mode is exposed with `aria-pressed`, never colour alone.
- **A swatch needs a boundary its fill does not supply.** Compute the real WCAG contrast of
  each swatch against the current `--panel` and draw a `--muted` ring when it is under
  3:1 (WCAG 1.4.11). The same flip picks black or white for a glyph on an arbitrary
  colour.
- **`--line` separates regions; `--muted` bounds controls.** `--line` is 1.38:1 against the
  panel, which is right for a rule between two areas and never enough for the edge of
  something you operate. Every button, field and chip takes its border from `--muted`,
  which clears 3:1 against the panel, the page and the pressed fill in both schemes.
- Icons are `<use>` references into the page's sprite, and every symbol in it is a
  vendored Bootstrap Icon copied verbatim (`docs/icons.md`) — never hand-drawn path data.
  `.ic` sets `fill: currentColor`, without which each glyph paints black and disappears on
  the dark panel.
- Disabled controls take `--muted` ink, a dashed border and the default cursor, so an
  unavailable control never advertises itself as available.
- No control below 44 × 44 at any breakpoint. The viewport stays zoomable;
  `touch-action: pinch-zoom` on the canvas. No gesture exists without a button or
  single-key equivalent.
- `prefers-reduced-motion` collapses all durations, and `prefers-color-scheme` supplies the
  dark tokens. High contrast is a toggle over `:root`, not a third stylesheet.

Which controls are on show follows `data-tool` on `<body>`, in CSS. Keep it that way: the
contextual toolbar then costs no JavaScript and cannot fall out of step with the tool.

Keep app navigation — Apps, undo, redo, contrast, the Sketch actions menu — in the top
bar's own `<nav>`, separate from the drawing controls. New sketch clears the shared canvas
and returns the colour row to its defaults; it does not introduce a document library. PNG
export renders the sheet at 1600 × 1200 whatever the zoom, on opaque white, without the
keyboard cursor.

## Undo, and the batch ceiling

Undo is shared like the canvas: every change a tab sees — its own, another window's,
another device's, and the log replayed at load — joins one history in arrival order, and
undo reverses the latest, bounded to 50 changes. A batch written as one act is undone as
one. Keep it that way: a per-tab undo on a replicated canvas surprises the person at the
other device. Restoring a deleted mark uses compensating events under a fresh id, because
a tombstoned id is never reused; the original `layer` travels with it so painting order
survives, and every reference held elsewhere — the other entries in both stacks, and any
fill anchored to the mark — is re-pointed through the map `compensate()` returns.

The node refuses a batch over `api.max_batch` and writes none of it
(`spec/data-api.md §3`). Anything that can name more marks than that — clearing a full
sheet, a large paste, a move over a big selection — is written in ceiling-sized chunks by
`commitGroups()`, which keeps a mark's tombstone and its replacement in the same chunk and
says in `#status` how many undo presses the action now takes.

Run `privatium lint apps/sketch` before finishing.
