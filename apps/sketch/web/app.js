/*
 * Project:  Privatium™  |  File: apps/sketch/web/app.js
 * Authors:  Gabriel Mongefranco (@gabrielmongefranco)
 * Created:  2026-08-28  |  Modified: 2026-09-05
 * Summary:  The whole app. Plain ES modules — no build step, no framework,
 *           no SQL. The event log is used directly as a document store. A
 *           stroke holds the pointer's capture from down to up, so ending
 *           it off the canvas still saves it; the keyboard draws too, and a
 *           live summary says what the canvas holds.
 */
import { pv } from '/static/pv.js';

const pad = document.getElementById('pad');
const ctx = pad.getContext('2d');
const status = document.getElementById('status');
const summary = document.getElementById('summary');
const NAMES = { '#00274C': 'navy', '#FFCB05': 'maize', '#333333': 'charcoal' };
let color = '#00274C';
let drawing = null;
function say(text) { status.textContent = text; }

// The CSS sizes the canvas (style.css); this matches the backing store to that size at
// the device's pixel ratio. Sizing from innerWidth instead would draw a 125 % or 200 %
// display's canvas past the viewport, and setting width resets the context, so the
// transform is set outright rather than scaled again on every resize.
function fit() {
  const r = devicePixelRatio || 1;
  pad.width = pad.clientWidth * r;
  pad.height = pad.clientHeight * r;
  ctx.setTransform(r, 0, 0, r, 0, 0);
  ctx.lineCap = ctx.lineJoin = 'round';
  redrawAll();
}
addEventListener('resize', fit);

// ---- rendering -----------------------------------------------------------
const strokes = new Map();          // id -> {points, color, width}

function paint(s) {
  ctx.strokeStyle = s.color;
  ctx.lineWidth = s.width;
  ctx.beginPath();
  s.points.forEach(([x, y], i) => (i ? ctx.lineTo(x, y) : ctx.moveTo(x, y)));
  ctx.stroke();
}

function redrawAll() {
  ctx.clearRect(0, 0, pad.clientWidth, pad.clientHeight);
  for (const s of strokes.values()) paint(s);
  if (drawing) paint(drawing);
  drawPen();
}

// The text of the drawing, for the summary the canvas is described by: how many strokes,
// in which colours. Refreshed whenever the set of strokes changes, from any source.
function summarize() {
  const counts = new Map();
  for (const s of strokes.values()) counts.set(s.color, (counts.get(s.color) || 0) + 1);
  const parts = [...counts].map(([c, n]) => `${n} ${NAMES[c] || c}`);
  summary.textContent = strokes.size
    ? `${strokes.size} stroke${strokes.size === 1 ? '' : 's'}: ${parts.join(', ')}.`
    : 'An empty canvas.';
}

// ---- input ---------------------------------------------------------------
// The canvas captures the pointer for the stroke, so a release outside it still ends the
// stroke here; a pointercancel or a lost capture ends it the same way. The stroke leaves
// the in-progress slot before the append is awaited, so one begun meanwhile is not
// cleared by this one's handler.
pad.addEventListener('pointerdown', e => {
  if (drawing) return;
  pad.setPointerCapture(e.pointerId);
  drawing = { points: [[e.offsetX, e.offsetY]], color, width: e.pressure ? e.pressure * 8 : 3 };
});

pad.addEventListener('pointermove', e => {
  if (!drawing) return;
  drawing.points.push([e.offsetX, e.offsetY]);
  paint({ ...drawing, points: drawing.points.slice(-2) });
});

async function finish() {
  if (!drawing) return;
  const stroke = drawing;
  drawing = null;
  const id = pv.ulid();
  strokes.set(id, stroke);
  summarize();
  // One append: a durable line in a text file you can read — and, from Phase 3, on every
  // device you own.
  await pv.put('stroke', id, stroke);
}
for (const end of ['pointerup', 'pointercancel', 'lostpointercapture']) pad.addEventListener(end, finish);

// ---- keyboard ------------------------------------------------------------
// A drawing must not need a pointer (WCAG 2.5.7, and the accessibility skill's rule that
// no drag has no single-key alternative). The canvas is focusable; while it has focus a
// dashed crosshair marks the pen. Arrow keys move it — Shift moves it farther — Space or
// Enter puts it down and lifts it, Escape discards the stroke in progress. What happens
// is said in the status region, so it is heard as well as seen.
const pen = { x: 40, y: 40 };
function drawPen() {
  if (document.activeElement !== pad) return;
  ctx.save();
  ctx.strokeStyle = color; ctx.lineWidth = 1; ctx.setLineDash([3, 3]);
  ctx.beginPath();
  ctx.moveTo(pen.x - 10, pen.y); ctx.lineTo(pen.x + 10, pen.y);
  ctx.moveTo(pen.x, pen.y - 10); ctx.lineTo(pen.x, pen.y + 10);
  ctx.stroke();
  ctx.restore();
}
pad.addEventListener('focus', () => { say('Arrow keys move the pen, Shift farther; Space puts it down and lifts it; Escape discards.'); redrawAll(); });
pad.addEventListener('blur', () => { say(''); redrawAll(); });
pad.addEventListener('keydown', e => {
  const step = e.shiftKey ? 25 : 5;
  let dx = 0, dy = 0;
  switch (e.key) {
    case 'ArrowLeft': dx = -step; break;
    case 'ArrowRight': dx = step; break;
    case 'ArrowUp': dy = -step; break;
    case 'ArrowDown': dy = step; break;
    case ' ':
    case 'Enter':
      e.preventDefault();
      if (drawing) { finish(); say('Pen up. Stroke saved.'); }
      else { drawing = { points: [[pen.x, pen.y]], color, width: 3 }; say('Pen down.'); }
      redrawAll();
      return;
    case 'Escape':
      if (drawing) { drawing = null; say('Stroke discarded.'); redrawAll(); }
      return;
    default:
      return;
  }
  e.preventDefault();
  pen.x = Math.max(0, Math.min(pad.clientWidth, pen.x + dx));
  pen.y = Math.max(0, Math.min(pad.clientHeight, pen.y + dy));
  if (drawing) drawing.points.push([pen.x, pen.y]);
  redrawAll();
});

// The current colour is state the page shows, not only a variable: aria-pressed on the
// swatch is what a screen reader announces and what style.css draws the ring from.
const swatches = document.querySelectorAll('.swatch');
swatches.forEach(b =>
  b.onclick = () => {
    color = b.dataset.color;
    swatches.forEach(s => s.setAttribute('aria-pressed', String(s === b)));
  });

document.getElementById('clear').onclick = async () => {
  const ids = [...strokes.keys()];
  strokes.clear();
  summarize();
  redrawAll();
  await pv.append(ids.map(id => ({ op: 'del', tbl: 'stroke', id })));
};

// ---- live updates --------------------------------------------------------
// Every stroke drawn in another window arrives here now; from Phase 3, every stroke from
// any paired device, including ones that reached this node from another node while this
// tab was closed.
pv.subscribe(ev => {
  if (ev.tbl !== 'stroke') return;
  if (ev.op === 'del') { strokes.delete(ev.id); redrawAll(); }
  else { strokes.set(ev.id, ev.d); paint(ev.d); }
  summarize();
});

pv.on('offline', () => say('Offline — your strokes are queued.'));
pv.on('online',  () => say(''));

// ---- boot ----------------------------------------------------------------
// The log in order: a put is a stroke, a del takes it back. The same read serves a
// resync, which is the node saying its cache was rebuilt underneath us.
async function load() {
  strokes.clear();
  for await (const ev of pv.events({ tbl: 'stroke' })) {
    if (ev.op === 'del') strokes.delete(ev.id); else strokes.set(ev.id, ev.d);
  }
  summarize();
  redrawAll();
}
pv.on('resync', load);
await load();
fit();
