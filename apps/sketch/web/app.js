/*
 * Project:  Privatium™  |  File: apps/sketch/web/app.js
 * Authors:  Gabriel Mongefranco (@gabrielmongefranco)
 * Created:  2026-08-28  |  Modified: 2026-09-07
 * Summary:  Drawing, controls and event replay. Plain ES modules — no build step, no
 *           framework, no SQL. The event log is used directly as a document store. A stroke
 *           holds the pointer's capture from down to up, so ending it off the canvas still
 *           saves it; the keyboard draws too, and a live summary says what the canvas
 *           holds. See main README.md for full license information.
 */
import { pv } from '/static/pv.js';
import { hitsStroke } from './strokes.js';
import { SketchHistory } from './history.js';

const pad = document.getElementById('pad');
const ctx = pad.getContext('2d');
const status = document.getElementById('status');
const summary = document.getElementById('summary');
const NAMES = { '#00274C': 'navy', '#FFCB05': 'maize', '#333333': 'charcoal', '#B42335': 'red', '#147D64': 'green', '#2459CF': 'blue', '#FFFFFF': 'white eraser' };
let color = '#00274C';
let mode = 'draw';
let erasePointer = null;
let drawing = null;
function say(text) { status.textContent = text; }

const exit = document.getElementById('exit');
exit.href = pv.url(pv.mount === '/' ? 'settings' : '../../');
const exitLabel = pv.mount === '/' ? 'Settings' : 'Apps';
exit.setAttribute('aria-label', exitLabel);
exit.title = exitLabel;
exit.hidden = false;

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

// ---- rendering -----------------------------------------------------------
const history = new SketchHistory(events => pv.append(events), () => pv.ulid());
const strokes = history.strokes;
const undo = document.getElementById('undo');
function refresh() { summarize(); redrawAll(); undo.disabled = !history.undoStack.length; }
function closePanels() { document.querySelectorAll('details[open]').forEach(el => { el.open = false; }); }
function newStroke(x, y, width = 3) {
  closePanels();
  return { points:[[x, y]], color:mode === 'erase' ? '#FFFFFF' : color, width:mode === 'erase' ? 24 : width };
}

function paint(s, context = ctx) {
  context.strokeStyle = s.color;
  context.lineWidth = s.width;
  context.beginPath();
  if (s.points.length === 1) {
    context.fillStyle = s.color;
    context.arc(s.points[0][0], s.points[0][1], s.width / 2, 0, Math.PI * 2);
    context.fill();
    return;
  }
  s.points.forEach(([x, y], i) => (i ? context.lineTo(x, y) : context.moveTo(x, y)));
  context.stroke();
}

function redrawAll() {
  ctx.clearRect(0, 0, pad.clientWidth, pad.clientHeight);
  for (const [, s] of history.entries()) paint(s);
  if (drawing) paint(drawing);
  drawPen();
}

// The text of the drawing, for the summary the canvas is described by: how many strokes,
// in which colors. Refreshed whenever the set of strokes changes, from any source.
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
  if (history.busy || e.button !== 0) return;
  closePanels();
  if (mode === 'stroke') { erasePointer = e.pointerId; return; }
  if (drawing) return;
  pad.setPointerCapture(e.pointerId);
  drawing = newStroke(e.offsetX, e.offsetY, e.pressure ? e.pressure * 8 : 3);
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
  try {
    const result = await history.change([{op:'put', tbl:'stroke', id, d:stroke}]);
    say(result?.queued ? 'Offline — stroke queued.' : 'Stroke saved.');
  } catch (error) { say(`Could not save stroke. ${error.message}`); }
  refresh();
}
pad.addEventListener('pointerup', e => {
  if (erasePointer === e.pointerId) {
    erasePointer = null;
    eraseAt(e.offsetX, e.offsetY);
  } else finish();
});
for (const end of ['pointercancel', 'lostpointercapture']) pad.addEventListener(end, () => {
  erasePointer = null;
  finish();
});

// A tombstone removes the topmost hit stroke from every window without rewriting it.
async function eraseAt(x, y) {
  const hit = history.entries().reverse().find(([, stroke]) => hitsStroke(stroke, x, y));
  if (!hit) { say('No stroke here to erase.'); return; }
  try {
    const result = await history.change([{ op: 'del', tbl: 'stroke', id: hit[0] }]);
    refresh();
    say(result?.queued ? 'Offline — erasure queued.' : 'Stroke erased.');
  } catch {
    say('Could not erase the stroke. Try again.');
  }
}

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
  ctx.strokeStyle = '#00274C'; ctx.lineWidth = 1; ctx.setLineDash([3, 3]);
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
      if (history.busy) return;
      closePanels();
      if (mode === 'stroke') { eraseAt(pen.x, pen.y); return; }
      if (drawing) { finish(); }
      else { drawing = newStroke(pen.x, pen.y); say('Pen down.'); }
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

// The current color is state the page shows, not only a variable: aria-pressed on the
// swatch is what a screen reader announces and what style.css draws the ring from.
const swatches = document.querySelectorAll('.swatch');
const picker = document.getElementById('color');
const hex = document.getElementById('hex');
const eraser = document.getElementById('eraser');
const wholeStroke = document.getElementById('erase-stroke');
const draw = document.getElementById('draw');
draw.onclick = () => chooseColor(color);
function chooseColor(value) {
  if (!/^#[0-9a-f]{6}$/i.test(value)) {
    document.getElementById('color-error').textContent = 'Enter # followed by six hexadecimal digits, such as #008080.';
    hex.setAttribute('aria-invalid', 'true');
    return;
  }
  color = value.toUpperCase();
  picker.value = color;
  hex.value = color;
  hex.removeAttribute('aria-invalid');
  document.getElementById('color-error').textContent = '';
  setMode('draw');
  swatches.forEach(s => s.setAttribute('aria-pressed', String(s.dataset.color === color)));
  say(`Pen color: ${NAMES[color] || color}.`);
}
picker.addEventListener('input', () => chooseColor(picker.value));
hex.addEventListener('change', () => chooseColor(hex.value));
hex.addEventListener('keydown', e => { if (e.key === 'Enter') chooseColor(hex.value); });
function setMode(value) {
  drawing = null;
  mode = value;
  draw.setAttribute('aria-pressed', String(mode === 'draw'));
  eraser.setAttribute('aria-pressed', String(mode === 'erase'));
  wholeStroke.setAttribute('aria-pressed', String(mode === 'stroke'));
  swatches.forEach(s => s.setAttribute('aria-pressed', String(mode === 'draw' && s.dataset.color === color)));
  say(mode === 'erase' ? 'Eraser: draw with white ink.' : mode === 'stroke'
    ? 'Stroke eraser: tap a stroke or press Space over it.' : 'Draw selected.');
  redrawAll();
}
eraser.onclick = () => setMode('erase');
wholeStroke.onclick = () => setMode('stroke');
swatches.forEach(b =>
  b.onclick = () => chooseColor(b.dataset.color));

undo.onclick = async () => {
  if (drawing) { drawing = null; refresh(); say('Stroke in progress discarded.'); return; }
  try {
    const result = await history.undo();
    refresh();
    say(result?.queued ? 'Offline — undo queued.' : 'Change undone.');
  } catch (error) { say(error.message); }
};
document.getElementById('clear').onclick = async () => {
  drawing = null;
  try {
    const result = await history.change([...strokes.keys()].map(id => ({op:'del', tbl:'stroke', id})));
    refresh();
    closePanels();
    pad.focus();
    say(result?.queued ? 'Offline — new sketch queued.' : 'New sketch ready. Undo restores the previous drawing.');
  } catch (error) { say(`Could not start a new sketch. ${error.message}`); }
};
// Render saved strokes on an opaque white surface; omit the keyboard crosshair.
document.getElementById('download').onclick = () => {
  const output = document.createElement('canvas');
  const entries = history.entries();
  let width = pad.clientWidth, height = pad.clientHeight;
  for (const [, stroke] of entries) {
    for (const [x, y] of stroke.points) {
      width = Math.max(width, x + stroke.width / 2);
      height = Math.max(height, y + stroke.width / 2);
    }
  }
  width = Math.ceil(width); height = Math.ceil(height);
  if (!Number.isFinite(width) || !Number.isFinite(height) || width * height > 16000000) {
    say('Drawing is too large to export. PNG export is limited to 16 million pixels.'); return;
  }
  output.width = width; output.height = height;
  const context = output.getContext('2d');
  context.fillStyle = '#FFFFFF'; context.fillRect(0, 0, width, height);
  context.lineCap = context.lineJoin = 'round';
  entries.forEach(([, stroke]) => paint(stroke, context));
  output.toBlob(blob => {
    if (!blob) { say('Could not create PNG. Try again.'); return; }
    const link = document.createElement('a');
    const url = URL.createObjectURL(blob);
    link.href = url; link.download = 'sketch.png'; link.click();
    setTimeout(() => URL.revokeObjectURL(url), 60000);
    say('PNG download requested.');
  }, 'image/png');
};

// ---- live updates --------------------------------------------------------
// Every stroke drawn in another window on this node arrives here, and once nodes sync
// so does every stroke from a paired device, including ones that landed while this tab
// was closed. Each joins the undo history like a stroke drawn here: the canvas is
// shared, so undo reverses the latest change to it whichever device made it.
pv.subscribe(ev => {
  if (ev.tbl !== 'stroke') return;
  history.apply(ev);
  refresh();
});

pv.on('offline', () => say('Offline — your strokes are queued.'));
pv.on('online',  () => say(''));

// ---- boot ----------------------------------------------------------------
// The log in order: a put is a stroke, a del takes it back, and the last fifty changes
// are what undo can reverse from the start. The same read serves a resync, which is
// the node saying its cache was rebuilt underneath us.
async function load() {
  strokes.clear();
  history.order.clear();
  history.undoStack.length = 0;
  for await (const ev of pv.events({ tbl: 'stroke' })) {
    history.apply(ev);
  }
  refresh();
}
pv.on('resync', load);
await load();
fit();
// Help, status text and wrapping tools can resize the canvas without a window resize.
new ResizeObserver(fit).observe(pad);
