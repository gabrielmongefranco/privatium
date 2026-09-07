// Project: Privatium™ | File: crates/privatium-core/tests/js/sketch-redesign.test.mjs
// Authors: Gabriel Mongefranco (@gabrielmongefranco)
// Created: 2026-09-07 | Modified: 2026-09-07
// Summary: The Sketch modules that carry no DOM: the fixed sheet's coordinate mapping and
//          zoom, mark geometry and hit testing, the tool and contrast tables, the SVG the
//          clipboard and the export write, redo, and the batch ceiling a write is chunked
//          at. Everything here runs the app's own code, not a copy of it.
//          See main README.md for full license information.
import { test } from 'node:test';
import assert from 'node:assert/strict';

import { Sheet, SHEET_W, SHEET_H } from '../../../../apps/sketch/web/sheet.js';
import { bounds, hits, inBox, isShape, moved, origin, textSize } from '../../../../apps/sketch/web/strokes.js';
import { dashFor } from '../../../../apps/sketch/web/paint.js';
import { toSvg, isOurs } from '../../../../apps/sketch/web/clip.js';
import { BASIC, DEFAULT_SLOTS, INKING, INKS, SIZES, TOOLS, colorName, contrast, inkOn, isHex, luminance, needsEdge } from '../../../../apps/sketch/web/tools.js';
import { MAX_BATCH, batches, SketchHistory } from '../../../../apps/sketch/web/history.js';

// ---- the sheet ------------------------------------------------------------------------
// Only what sheet.js actually touches: a canvas with a box, a scrolling viewport, a zoom
// key that survives a reload, and a device pixel ratio.
function stage({ view = [1200, 900], ratio = 1, stored = null } = {}) {
  const store = new Map(stored === null ? [] : [['sketch.zoom', stored]]);
  globalThis.localStorage = {
    getItem: key => (store.has(key) ? store.get(key) : null),
    setItem: (key, value) => store.set(key, String(value)),
    removeItem: key => store.delete(key)
  };
  globalThis.window = { devicePixelRatio: ratio };
  const canvas = {
    width: SHEET_W, height: SHEET_H, style: {},
    getBoundingClientRect: () => ({
      left: 40, top: 20,
      width: parseFloat(canvas.style.width), height: parseFloat(canvas.style.height)
    })
  };
  const viewport = {
    clientWidth: view[0], clientHeight: view[1],
    scrollWidth: view[0], scrollHeight: view[1], scrollLeft: 0, scrollTop: 0
  };
  return { sheet: new Sheet(canvas, viewport), canvas, viewport, store };
}

test('the sheet is a fixed coordinate space, whatever the window', () => {
  // A wide desktop and a narrow phone map the same pointer position to the same sheet
  // coordinate. Sizing the canvas to each window instead is what put a desktop drawing in
  // the corner of a phone.
  const seen = [];
  for (const [view, ratio] of [[[1440, 900], 1], [[390, 780], 3], [[3000, 2000], 2]]) {
    const { sheet, canvas } = stage({ view, ratio });
    sheet.resize();
    const r = canvas.getBoundingClientRect();
    seen.push(sheet.at({ clientX: r.left + r.width * 0.25, clientY: r.top + r.height * 0.75 }));
  }
  for (const point of seen) {
    assert.ok(Math.abs(point.x - SHEET_W * 0.25) < 1, `x was ${point.x}`);
    assert.ok(Math.abs(point.y - SHEET_H * 0.75) < 1, `y was ${point.y}`);
  }
});

test('the backing store follows the CSS box at the device ratio and never falls below the sheet', () => {
  // A window large enough to show the sheet at its own size: the store is the CSS box.
  const { sheet, canvas } = stage({ view: [2000, 1500], ratio: 1 });
  sheet.resize();
  assert.equal(canvas.width, Math.round(parseFloat(canvas.style.width)));
  assert.equal(canvas.width / SHEET_W, sheet.k);
  assert.ok(sheet.k >= 1, 'the store never carries less than the sheet');

  // A smaller one: the CSS box shrinks to fit, and the store holds at the sheet's own
  // resolution so the drawing keeps its detail.
  const fitted = stage({ view: [1200, 900], ratio: 1 });
  fitted.sheet.resize();
  assert.ok(parseFloat(fitted.canvas.style.width) < SHEET_W);
  assert.equal(fitted.canvas.width, SHEET_W);
  assert.equal(fitted.canvas.height, SHEET_H);
  assert.equal(fitted.sheet.k, 1);

  // A phone: the CSS box is tiny, so the store is held at the sheet's own resolution
  // rather than throwing away the drawing's detail.
  const small = stage({ view: [360, 640], ratio: 3 });
  small.sheet.resize();
  assert.ok(small.canvas.width >= SHEET_W && small.canvas.height >= SHEET_H);
  assert.equal(small.sheet.k, small.canvas.width / SHEET_W);

  // Zoomed on a HiDPI display: the store is larger than the sheet, so a zoomed sheet is
  // sharp rather than upscaled.
  const zoomed = stage({ view: [1600, 1200], ratio: 2 });
  zoomed.sheet.setZoom(2);
  zoomed.sheet.resize();
  assert.equal(zoomed.canvas.width, SHEET_W * 4);
  assert.equal(zoomed.sheet.k, 4);
});

test('a pixel-space point follows the same scale as the drawing', () => {
  const { sheet } = stage({ view: [1600, 1200], ratio: 2 });
  sheet.setZoom(1);
  sheet.resize();
  assert.deepEqual(sheet.toDevice({ x: 800, y: 600 }), { x: 800 * sheet.k, y: 600 * sheet.k });
});

test('zoom is per-device view state, clamped, stepped in fives and never written to a log', () => {
  const { sheet, store } = stage();
  assert.equal(sheet.zoom, 0, 'absent means fit');

  sheet.setZoom(9);
  assert.equal(sheet.zoom, 4, 'clamped to 400 per cent');
  sheet.setZoom(0.001);
  assert.equal(sheet.zoom, 0.05, 'clamped to 5 per cent');
  assert.equal(store.get('sketch.zoom'), '0.05');

  sheet.setZoom(0);
  assert.equal(store.has('sketch.zoom'), false, 'fit clears the key rather than storing a scale');

  sheet.setZoom(1);
  sheet.step(1);
  assert.equal(sheet.percent, 105);
  sheet.step(-1);
  assert.equal(sheet.percent, 100);

  // A stored zoom is read back; a nonsensical one falls back to fit.
  assert.equal(stage({ stored: '1.5' }).sheet.zoom, 1.5);
  assert.equal(stage({ stored: 'banana' }).sheet.zoom, 0);
  assert.equal(stage({ stored: '99' }).sheet.zoom, 0);
});

test('fit leaves room for the sheet border and never returns a scale of zero', () => {
  const { sheet } = stage({ view: [1628, 1228] });
  assert.equal(sheet.scale, 1);
  assert.ok(stage({ view: [0, 0] }).sheet.scale > 0, 'a collapsed viewport still has a scale');
});

// ---- geometry -------------------------------------------------------------------------
const freehand = { points: [[100, 100], [200, 200]], color: '#000000', width: 6 };
const rect = { kind: 'rect', a: { x: 100, y: 100 }, b: { x: 300, y: 200 }, color: '#000000', width: 6 };
const text = { kind: 'text', x: 100, y: 100, text: 'Label', color: '#000000', width: 6 };

test('a mark is classified by what it is, not by which fields it carries', () => {
  assert.equal(isShape(rect), true);
  assert.equal(isShape(freehand), false, 'no kind means freehand');
  assert.equal(isShape({ kind: 'text' }), false);
  assert.equal(isShape(null), false);
});

test('bounds refuses unusable geometry rather than returning a box around nothing', () => {
  for (const bad of [
    null,
    { points: [[1, 1]] },                                    // no width
    { width: 0, points: [[1, 1]] },
    { width: 6, points: [[NaN, 1]] },
    { kind: 'rect', width: 6, a: { x: 1, y: 1 } },           // no second corner
    { kind: 'text', width: 6, x: 1, y: 1 },                  // no words
    { kind: 'fill', width: 6, x: 1, y: 1 }                   // a region, not an object
  ]) {
    assert.equal(bounds(bad), null, JSON.stringify(bad));
  }
  assert.deepEqual(bounds(rect), [89, 89, 311, 211], 'padded by half the width and a margin');
});

test('a click selects a freehand mark by its painted path and a shape by its box', () => {
  // Inside the loop of a curve is not on the curve, so it does not grab it.
  const loop = { points: [[0, 0], [200, 0], [200, 200], [0, 200], [0, 0]], width: 4 };
  assert.equal(hits(loop, 100, 100), false);
  assert.equal(hits(loop, 100, 0), true);
  // A shape and a text block are grabbed anywhere in the box their outline shows.
  assert.equal(hits(rect, 200, 150), true);
  assert.equal(hits(rect, 400, 150), false);
  assert.equal(hits(text, 105, 100), true);
  assert.equal(hits({ kind: 'fill', x: 1, y: 1, width: 6 }, 1, 1), false, 'a fill is never selected');
});

test('a marquee takes every mark its box touches, and nothing it does not', () => {
  assert.equal(inBox(rect, { x: 0, y: 0 }, { x: 150, y: 150 }), true, 'a partial overlap counts');
  assert.equal(inBox(rect, { x: 400, y: 400 }, { x: 500, y: 500 }), false);
  assert.equal(inBox({ kind: 'fill', x: 1, y: 1 }, { x: 0, y: 0 }, { x: 9, y: 9 }), false);
});

test('moving a mark moves every coordinate it has, including a fill\'s anchor point', () => {
  const pressure = { points: [[10, 10, 3], [20, 20]], width: 6 };
  assert.deepEqual(moved(pressure, 5, -5).points, [[15, 5, 3], [25, 15]],
    'a per-point width survives, and a two-number point stays two numbers');
  assert.deepEqual(moved(rect, 10, 10).a, { x: 110, y: 110 });
  const fill = { kind: 'fill', x: 150, y: 150, anchor: 'shape', anchorAt: { x: 100, y: 100 } };
  const shifted = moved(fill, 30, 40);
  assert.deepEqual([shifted.x, shifted.y], [180, 190]);
  assert.deepEqual(shifted.anchorAt, { x: 130, y: 140 }, 'the anchor travels with the fill');
  assert.equal(shifted.anchor, 'shape', 'the shape it belongs to is unchanged');
  assert.deepEqual(fill.anchorAt, { x: 100, y: 100 }, 'the original is not mutated');
});

test('a fill anchors to a shape\'s top-left corner however the shape was drawn', () => {
  const backwards = { kind: 'rect', a: { x: 300, y: 200 }, b: { x: 100, y: 100 }, width: 6 };
  assert.deepEqual(origin(backwards), { x: 100, y: 100 });
  assert.equal(origin(freehand), null, 'only a shape carries an anchor');
});

test('one control sets a text mark\'s size, and the size floors so text stays legible', () => {
  assert.equal(textSize({ width: 32 }), 160);
  assert.equal(textSize({ width: 2 }), 28);
  assert.equal(textSize({}), 30);
});

test('a dash pattern scales with the stroke so it reads the same at Fine and at Broad', () => {
  assert.deepEqual(dashFor(2, 'dashed'), [6, 4]);
  assert.deepEqual(dashFor(32, 'dashed'), [96, 64]);
  const dotted = dashFor(6, 'dotted');
  assert.equal(dotted[0], 1);
  assert.ok(Math.abs(dotted[1] - 13.2) < 1e-9, String(dotted[1]));
  assert.deepEqual(dashFor(6, 'solid'), [], 'a solid line has no pattern');
});

// ---- the tool and colour tables ---------------------------------------------------------
test('every tool has a unique letter and an icon the framework ships', () => {
  const keys = TOOLS.map(t => t.key);
  assert.equal(new Set(keys).size, TOOLS.length, 'no two tools answer the same key');
  assert.equal(TOOLS.length, 10);
  for (const tool of TOOLS) {
    assert.match(tool.icon, /^[a-z0-9-]+$/, tool.id);
    assert.ok(tool.label && tool.label !== tool.id, tool.id);
  }
  for (const id of BASIC.concat(INKING)) {
    assert.ok(TOOLS.some(t => t.id === id), `${id} is not a tool`);
  }
  assert.deepEqual(SIZES.map(s => s.px), [2, 6, 14, 32]);
});

test('the colour row starts with four inks and three empty slots, all editable', () => {
  assert.equal(DEFAULT_SLOTS.length, 7);
  assert.equal(DEFAULT_SLOTS.filter(Boolean).length, 4);
  assert.equal(colorName(''), 'empty');
  assert.equal(colorName('#ffb703'), 'Yellow', 'a known ink is named, not numbered');
  assert.equal(colorName('#123456'), '#123456', 'and an unknown one falls back to its hex');
  assert.equal(INKS.length, 12);
  for (const ink of INKS) assert.equal(isHex(ink.hex), true, ink.name);
});

test('a hex value is accepted only in the one form the app writes', () => {
  for (const good of ['#008080', ' #ABCDEF ', '#abcdef']) assert.equal(isHex(good), true, good);
  for (const bad of ['', '008080', '#0088', '#12345g', null, undefined, '#0000001']) {
    assert.equal(isHex(bad), false, String(bad));
  }
});

test('contrast is computed on linearised channels, so a bright colour is not read as dark', () => {
  // A plain channel average lets #FFB703 through as though it were dark. It is not.
  assert.ok(luminance('#FFB703') > 0.5, 'yellow is a light colour');
  assert.ok(Math.abs(contrast('#000000', '#FFFFFF') - 21) < 0.01);
  assert.equal(contrast('#123456', '#123456'), 1);
  assert.equal(inkOn('#FFB703'), '#000000', 'black reads on yellow');
  assert.equal(inkOn('#2E4C9E'), '#FFFFFF', 'white reads on ultramarine');
});

test('a swatch is given a boundary exactly when its own fill does not supply one', () => {
  const light = '#FFFCFA', dark = '#1E1720';
  assert.equal(needsEdge('#FFB703', light), true, 'yellow is 1.71:1 on the light panel');
  assert.equal(needsEdge('#FFFFFF', light), true);
  assert.equal(needsEdge('#000000', light), false);
  assert.equal(needsEdge('#000000', dark), true, 'and black is the one that needs it in the dark');
  assert.equal(needsEdge('#FFFFFF', dark), false);
  assert.equal(needsEdge('', light), true, 'an empty slot always shows its outline');
});

test('the app\'s own control boundary clears 3:1 on every surface it is drawn against', () => {
  // --line is 1.38:1 against the panel and separates regions; --muted bounds anything a
  // person operates and carries WCAG 1.4.11 (docs/sketch-app-design.md).
  for (const [muted, surfaces] of [
    ['#6B5F68', ['#FFFCFA', '#F6F1EE', '#EBCBF3', '#FFFFFF']],
    ['#B2A5B4', ['#1E1720', '#151016', '#6C4A79']]
  ]) {
    for (const surface of surfaces) {
      assert.ok(contrast(muted, surface) >= 3,
        `${muted} on ${surface} is ${contrast(muted, surface).toFixed(2)}:1`);
    }
  }
});

// ---- the SVG the clipboard and the export write ------------------------------------------
test('every kind of mark reaches SVG, and a fill is reported rather than guessed at', () => {
  const { text: svg, skipped } = toSvg([
    freehand,
    { ...freehand, points: [[0, 0], [50, 50], [100, 0]] },
    rect,
    { kind: 'line', a: { x: 0, y: 0 }, b: { x: 10, y: 10 }, color: '#E63946', width: 4, dash: 'dashed' },
    { kind: 'ellipse', a: { x: 0, y: 0 }, b: { x: 40, y: 20 }, color: '#457B9D', width: 2 },
    text,
    { kind: 'fill', x: 5, y: 5, color: '#FFB703' }
  ]);
  assert.equal(skipped, 1, 'a fill is pixels, not a shape');
  assert.match(svg, /^<svg xmlns="http:\/\/www\.w3\.org\/2000\/svg"/);
  assert.match(svg, new RegExp(`width="${SHEET_W}" height="${SHEET_H}"`));
  for (const element of ['<path ', '<rect ', '<line ', '<ellipse ', '<text ']) {
    assert.ok(svg.includes(element), `${element} is missing`);
  }
  assert.match(svg, /stroke-dasharray="12,8"/, 'the line style survives the export');
  assert.ok(!svg.includes('<svg', 1), 'one document, not a nest of them');
});

test('a translucent mark is exported as a group, which composites the way the canvas does', () => {
  const { text: svg } = toSvg([{ ...freehand, blend: 'multiply' }]);
  assert.match(svg, /<g style="mix-blend-mode:multiply">/);
  assert.match(svg, /<\/g>/);
});

test('a pressure stroke keeps its per-point widths, and an even one becomes one curve', () => {
  const even = toSvg([{ points: [[0, 0], [10, 10], [20, 0], [30, 10]], color: '#000000', width: 6 }]).text;
  assert.equal((even.match(/<path /g) || []).length, 1);
  assert.match(even, / Q /, 'curves through the midpoints, not a run of straight lines');

  const varying = toSvg([{ points: [[0, 0, 2], [10, 10, 9], [20, 0, 14]], color: '#000000', width: 6 }]).text;
  assert.equal((varying.match(/<line /g) || []).length, 2, 'a segment per pair, each at its own width');
  assert.match(varying, /stroke-width="5.5"/);
});

test('text in a mark is escaped, never emitted as markup', () => {
  const { text: svg } = toSvg([{ ...text, text: '<script>alert(1)</script> & "more"' }]);
  assert.ok(!svg.includes('<script>'), svg);
  assert.match(svg, /&lt;script&gt;alert\(1\)&lt;\/script&gt; &amp; /);
});

test('our own clipboard SVG is recognised by its stamp and nothing else is', () => {
  const stamp = 'sk' + (1234567890).toString(36);
  const { text: mine } = toSvg([freehand], stamp);
  assert.equal(isOurs(mine, stamp), true);
  assert.equal(isOurs(mine, 'sk-other'), false, 'a different session is a foreign payload');
  assert.equal(isOurs(toSvg([freehand]).text, stamp), false, 'an export carries no stamp');
  for (const empty of [null, '', undefined]) {
    assert.equal(isOurs(empty, stamp), false);
    assert.equal(isOurs(mine, empty), false);
  }
});

// ---- writing at the node's ceiling --------------------------------------------------------
test('a write is chunked at the ceiling and a group is never split across two batches', () => {
  assert.equal(MAX_BATCH, 1000, 'spec/data-api.md §3');

  const single = Array.from({ length: 2500 }, (_, i) => [{ op: 'del', id: String(i) }]);
  const parts = batches(single);
  assert.deepEqual(parts.map(p => p.length), [1000, 1000, 500]);
  assert.equal(parts.flat().length, 2500, 'nothing is dropped');

  // A move is a tombstone and a fresh put per mark. Splitting one across two writes would
  // leave a mark deleted if the second never lands.
  const pairs = Array.from({ length: 600 }, (_, i) => [{ op: 'del', id: `d${i}` }, { op: 'put', id: `p${i}` }]);
  const moved = batches(pairs);
  assert.deepEqual(moved.map(p => p.length), [1000, 200]);
  for (const part of moved) {
    for (let i = 0; i < part.length; i += 2) {
      assert.equal(part[i].op, 'del');
      assert.equal(part[i + 1].op, 'put', 'a tombstone and its replacement stay together');
    }
  }

  assert.deepEqual(batches([]), [], 'nothing to write is no batches at all');
  assert.deepEqual(batches([[{ op: 'del', id: 'a' }]]), [[{ op: 'del', id: 'a' }]]);
});

// ---- redo ----------------------------------------------------------------------------------
let minted = 0;
const history = write => new SketchHistory(write, () => `fresh-${minted++}`);
const put = (id, d = { points: [[1, 1], [2, 2]], color: '#000000', width: 3 }) =>
  ({ op: 'put', tbl: 'stroke', id, d });

test('redo puts back what undo reversed, under fresh ids, and is dropped by a new change', async () => {
  const written = [];
  const m = history(async events => { written.push(events.map(e => `${e.op} ${e.id}`)); });
  await m.change([put('a')]);
  await m.change([put('b')]);
  assert.equal(m.canUndo, true);
  assert.equal(m.canRedo, false);

  await m.undo();
  assert.deepEqual([...m.strokes.keys()], ['a']);
  assert.equal(m.canRedo, true);

  await m.redo();
  assert.equal(m.strokes.size, 2);
  assert.equal(m.canRedo, false);
  // Undo tombstoned 'b', and a tombstoned id is never the key of another row
  // (spec/protocol.md §4.6), so redo puts it back under a fresh one.
  const back = [...m.strokes.keys()].find(id => id !== 'a');
  assert.notEqual(back, 'b');
  assert.equal(m.strokes.get(back).layer, 'b', 'and it keeps its place in the painting order');
  assert.deepEqual(written.at(-1), [`put ${back}`]);

  // Any new change, from anywhere, drops the redo branch.
  await m.undo();
  assert.equal(m.canRedo, true);
  m.apply({ ...put('remote'), ts: '2026-09-07T10:00:00.000Z', dev: 'k7m2q9xf' });
  assert.equal(m.canRedo, false);
});

test('an undone deletion redoes as a deletion of whatever the undo restored', async () => {
  const written = [];
  const m = history(async events => { written.push(events.map(e => `${e.op} ${e.id}`)); });
  await m.change([put('shape')]);
  await m.change([{ op: 'del', tbl: 'stroke', id: 'shape' }]);

  await m.undo();
  const [[restoredId, restored]] = m.entries();
  assert.notEqual(restoredId, 'shape', 'a tombstoned id is never reused');
  assert.equal(restored.layer, 'shape', 'painting order survives');

  await m.redo();
  assert.equal(m.strokes.size, 0);
  assert.deepEqual(written.at(-1), [`del ${restoredId}`], 'redo deletes what is actually there');
});

test('a write is never mutated after it has been handed over', async () => {
  // pv.js queues what it was given when the node is unreachable, so re-pointing an id
  // must not reach back into an event that has already been written.
  const written = [];
  const m = history(async events => { written.push(events); });
  await m.change([put('one')]);
  await m.change([{ op: 'del', tbl: 'stroke', id: 'one' }]);
  const clear = written.map(events => events.map(e => `${e.op} ${e.id}`));
  await m.undo();
  await m.undo();
  assert.deepEqual(written.slice(0, 2).map(events => events.map(e => `${e.op} ${e.id}`)), clear);

  // And replaying every line the writer received reproduces the visible canvas exactly.
  const replay = history(async () => {});
  for (const events of written) for (const ev of events) replay.apply(ev);
  assert.deepEqual(replay.entries(), m.entries());
});

test('a fill anchored to a restored shape follows it to its new id', async () => {
  const m = history(async () => {});
  await m.change([put('shape', { kind: 'rect', a: { x: 0, y: 0 }, b: { x: 9, y: 9 }, color: '#000000', width: 6 })]);
  await m.change([put('paint', { kind: 'fill', x: 4, y: 4, color: '#FFB703', anchor: 'shape', anchorAt: { x: 0, y: 0 } })]);
  await m.change([{ op: 'del', tbl: 'stroke', id: 'shape' }]);
  await m.undo();
  const fresh = [...m.strokes.keys()].find(id => id !== 'paint');
  assert.notEqual(fresh, 'shape');
  assert.equal(m.strokes.get('paint').anchor, fresh,
    'or the fill would silently stop drawing, since an unknown anchor is not painted');
});
