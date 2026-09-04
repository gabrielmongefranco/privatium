/*
 * Project:  Privatium™  |  File: apps/sketch/web/app.js
 * Authors:  Gabriel Mongefranco (@gabrielmongefranco)
 * Created:  2026-08-28  |  Modified: 2026-09-04
 * Summary:  The whole app. Plain ES modules — no build step, no framework,
 *           no SQL. The event log is used directly as a document store.
 */
import { pv } from '/static/pv.js';

const pad = document.getElementById('pad');
const ctx = pad.getContext('2d');
const status = document.getElementById('status');
let color = '#00274C';
let drawing = null;

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
}

// ---- input ---------------------------------------------------------------
pad.addEventListener('pointerdown', e => {
  drawing = { points: [[e.offsetX, e.offsetY]], color, width: e.pressure ? e.pressure * 8 : 3 };
});

pad.addEventListener('pointermove', e => {
  if (!drawing) return;
  drawing.points.push([e.offsetX, e.offsetY]);
  paint({ ...drawing, points: drawing.points.slice(-2) });
});

pad.addEventListener('pointerup', async () => {
  if (!drawing) return;
  const id = pv.ulid();
  strokes.set(id, drawing);
  // One append. It is now on every device you own, in a text file you can read.
  await pv.put('stroke', id, drawing);
  drawing = null;
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
  redrawAll();
  await pv.append(ids.map(id => ({ op: 'del', tbl: 'stroke', id })));
};

// ---- sync ----------------------------------------------------------------
// Every stroke drawn on any paired device arrives here, including strokes that
// reached this node from another node over iroh while we were asleep.
pv.subscribe(ev => {
  if (ev.tbl !== 'stroke') return;
  if (ev.op === 'del') { strokes.delete(ev.id); redrawAll(); }
  else { strokes.set(ev.id, ev.d); paint(ev.d); }
});

pv.on('offline', () => status.textContent = 'Offline — your strokes are queued.');
pv.on('online',  () => status.textContent = '');

// ---- boot ----------------------------------------------------------------
// The log in order: a put is a stroke, a del takes it back. The same read serves a
// resync, which is the node saying its cache was rebuilt underneath us.
async function load() {
  strokes.clear();
  for await (const ev of pv.events({ tbl: 'stroke' })) {
    if (ev.op === 'del') strokes.delete(ev.id); else strokes.set(ev.id, ev.d);
  }
  redrawAll();
}
pv.on('resync', load);
await load();
fit();
