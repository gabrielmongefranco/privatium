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
| Colour inside a shape | `{ kind:'fill', shape:'rect'\|'ellipse'\|'free', a, b or points, color, anchor }` |
| The sheet's colour | `{ kind:'page', color }` |

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
  ratio), `paint.js` (drawing one mark and one filled area), `strokes.js` (geometry, hit
  testing and what a fill covers), `clip.js` (clipboard and SVG), `tools.js` (the tool,
  size, ink and contrast tables), `history.js` (undo and redo).
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
the one operation that is unavoidably in pixels, the eyedropper.

Zoom is per-device view state in `localStorage` under `sketch.zoom`; it must never reach
the log. Buttons step 5 points at a time, the percentage is editable, and 0 fits.

## Tools

Ten: Select, Brush, Eraser, Eyedropper, Fill, Line, Rectangle, Ellipse, Text, Pan.

- **Select** moves and resizes whole marks, never a rectangle of pixels — a pixel marquee
  cannot be expressed as edits to the log, and the canvas is shared. Stroke mode steps
  through overlapping marks on a repeat click; Rectangle mode takes every mark its box
  touches. A move or a resize is a tombstone and a fresh put per mark, carrying `layer`,
  written as one batch so one undo returns it.
- **Moving and resizing are one operation with different transforms**, both built by
  `strokes.rewriteEvents`. Keep them there: it is the single place a fill is carried along
  with its shape, and a second path is how a fill ends up with a transform of its own.
  Measure a resize with `extent`, never `bounds` — `bounds` pads by the stroke width, and a
  constant pad does not scale, so the shape drifts away from the pointer. An outline keeps
  its `width` when scaled; only text scales its size, uniformly by the smaller factor.
  The handle is a drag, so `Ctrl`/`⌘` with an arrow and the rail's Bigger and Smaller
  buttons exist beside it (WCAG 2.5.7).
- **Fill is the inside of one mark, not a flood.** It carries its own geometry —
  `strokes.areaOf` computes it, `strokes.areaFilled` reads it back — so nothing is looked
  up to draw it. `anchor` names the mark the colour belongs to and is used only so a move
  and a deletion carry it along; a stale anchor means the colour does not follow, never
  colour across the sheet. **Do not reintroduce a seed.** A seed is not an object: it
  cannot be selected or moved, and what it covers changes whenever anything is drawn near
  it, which is where every fill bug in this app came from. Copying a shape brings its
  colour; restoring one re-points it (`history.js` returns the id map).
- Tapping Fill away from every shape colours the sheet, as `{ kind:'page', color }`. The
  last one wins, it is the surface rather than a mark in the painting order, and it is
  never selectable.
- A mark that holds a fill is grabbed anywhere inside it. Freehand is otherwise hit-tested
  against the painted path, which is what stops a loose scribble grabbing everything in
  its bounding box — but a coloured-in circle you can only grab by the outline is wrong.
- A drag repaints the sheet as the finished move will look, colours included, at most once
  a frame — a pointer reports faster than the display refreshes.
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
- **SVG export** writes a fill anchored to a rectangle or an ellipse as that shape's own
  area, inset by half the outline; a fill on the open page has no shape behind it, so it is
  counted and left out. `toSvg` takes `[id, mark]` pairs because a fill has to find the
  shape it belongs to.
- **The four quick tools are split buttons**: the tool, and a corner caret that opens its
  options. Long press, right click, `ArrowDown` and `Shift+F10` reach the same menu, but
  the caret is a control of its own — a gesture never gets to be the only way in (WCAG
  2.5.7), and every option in a menu is also a full-size button in the rail.

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
contextual toolbar then costs no JavaScript and cannot fall out of step with the tool. It
does mean a delegated click handler must name the control — `closest('button[data-tool]')`,
never `closest('[data-tool]')`, which matches `<body>` from anywhere on the page and
re-chooses the current tool on every click of the sheet.

Keep app navigation — Apps, undo, redo, contrast, the Sketch actions menu — in the top
bar's own `<nav>`, separate from the drawing controls. New sketch clears the shared canvas
and returns the colour row to its defaults; it does not introduce a document library. PNG
export renders the sheet at 1600 × 1200 whatever the zoom, on opaque white, without the
keyboard cursor.

## Undo, and the batch ceiling

A tab's own write arrives back on the stream, and can do so before the call that wrote it
returns. `history.js` holds the events of the write in flight and applies their echo
without remembering it; counting it as a change of its own remembers every mark twice and
leaves the second undo refusing forever.

Undo is shared like the canvas: every change a tab sees — its own, another window's,
another device's, and the log replayed at load — joins one history in arrival order, and
undo reverses the latest, bounded to 50 changes. A batch written as one act is undone as
one. Keep it that way: a per-tab undo on a replicated canvas surprises the person at the
other device. Restoring a deleted mark uses compensating events under a fresh id, because
a tombstoned id is never reused; the original `layer` travels with it so painting order
survives, and every reference held elsewhere — the other entries in both stacks, and any
fill anchored to the mark — is re-pointed through the map `compensate()` returns.

The node refuses a batch over `api.max_batch` and writes none of it
(`spec/data-api.md §2`, tabled in `§7`). Anything that can name more marks than that — clearing a full
sheet, a large paste, a move over a big selection — is written in ceiling-sized chunks by
`commitGroups()`, which keeps a mark's tombstone and its replacement in the same chunk and
says in `#status` how many undo presses the action now takes.

Run `privatium lint apps/sketch` before finishing.
