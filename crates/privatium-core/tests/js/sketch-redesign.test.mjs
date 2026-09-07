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
import { areaFilled, areaOf, bounds, covers, encloses, hits, inBox, isShape, moveEvents, moved, textSize } from '../../../../apps/sketch/web/strokes.js';
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
  const fill = { kind: 'fill', shape: 'rect', a: { x: 100, y: 100 }, b: { x: 200, y: 160 }, anchor: 'shape' };
  const shifted = moved(fill, 30, 40);
  assert.deepEqual(shifted.a, { x: 130, y: 140 });
  assert.deepEqual(shifted.b, { x: 230, y: 200 });
  assert.equal(shifted.anchor, 'shape', 'the mark it belongs to is unchanged');
  assert.deepEqual(fill.a, { x: 100, y: 100 }, 'the original is not mutated');
});

test('a mark that encloses an area can be coloured inside; one that encloses nothing cannot', () => {
  assert.equal(encloses(rect), true);
  assert.equal(encloses({ kind: 'ellipse', a: { x: 0, y: 0 }, b: { x: 9, y: 9 }, width: 6 }), true);
  assert.equal(encloses(freehand), true, 'a hand-drawn outline encloses an area');
  assert.equal(encloses({ kind: 'line', a: { x: 0, y: 0 }, b: { x: 9, y: 9 }, width: 6 }), false);
  assert.equal(encloses(text), false);
  assert.equal(encloses({ kind: 'fill', shape: 'rect', a: rect.a, b: rect.b }), false);
  assert.equal(encloses({ kind: 'page', color: '#F6F1EE' }), false, 'the sheet is not a mark on itself');
  assert.equal(encloses(null), false);
});

test('a click is inside a mark when it is inside the box the selection outline shows', () => {
  assert.equal(covers(rect, 200, 150), true);
  assert.equal(covers(rect, 400, 150), false);
  assert.equal(covers(rect, NaN, 150), false);
  assert.equal(covers({ kind: 'page', color: '#000000' }, 1, 1), false, 'no box, no claim');
});

test('the inside of a shape stops where its outline does', () => {
  // The colour goes up to the inner edge of the stroke, not to the path the stroke
  // straddles, so a thick outline leaves less room inside it.
  assert.deepEqual(areaOf(rect), { shape: 'rect', a: { x: 103, y: 103 }, b: { x: 297, y: 197 } });
  assert.deepEqual(
    areaOf({ kind: 'ellipse', a: { x: 100, y: 60 }, b: { x: 0, y: 0 }, width: 4 }),
    { shape: 'ellipse', a: { x: 2, y: 2 }, b: { x: 98, y: 58 } },
    'the same either way round'
  );
  // A hand-drawn outline is its own samples, which is what the canvas draws it as.
  const ring = { points: [[0, 0], [10, 10], [0, 20]], width: 6 };
  assert.deepEqual(areaOf(ring), { shape: 'free', points: [[0, 0], [10, 10], [0, 20]] });
  assert.notEqual(areaOf(ring).points, ring.points, 'a copy, so moving the fill cannot move the mark');

  // Nothing left inside once the outline is taken off, and nothing that encloses nothing.
  assert.equal(areaOf({ kind: 'rect', a: { x: 0, y: 0 }, b: { x: 5, y: 5 }, width: 32 }), null);
  assert.equal(areaOf({ points: [[0, 0], [9, 9]], width: 6 }), null, 'two samples enclose nothing');
  assert.equal(areaOf({ kind: 'line', a: { x: 0, y: 0 }, b: { x: 9, y: 9 }, width: 6 }), null);
});

test('a fill carries its own area, and an older one is read from the mark it named', () => {
  const carried = { kind: 'fill', shape: 'rect', a: { x: 1, y: 2 }, b: { x: 3, y: 4 }, color: '#FFB703' };
  assert.deepEqual(areaFilled(carried, null), { shape: 'rect', a: { x: 1, y: 2 }, b: { x: 3, y: 4 } },
    'nothing is looked up to draw it');
  const curved = { kind: 'fill', shape: 'free', points: [[0, 0], [9, 0], [9, 9]], color: '#FFB703' };
  assert.deepEqual(areaFilled(curved, null).points, [[0, 0], [9, 0], [9, 9]]);

  // A fill from a log written when a fill was a seed: it becomes the inside of the mark
  // it was anchored to, which is what it was meant to be.
  const legacy = { kind: 'fill', x: 200, y: 150, color: '#FFB703', anchor: 'r', anchorAt: { x: 100, y: 100 } };
  assert.deepEqual(areaFilled(legacy, rect), areaOf(rect));
  assert.equal(areaFilled(legacy, null), null, 'and one whose mark is gone draws nothing');
  assert.equal(areaFilled({ kind: 'fill', x: 5, y: 5, color: '#FFB703' }, null), null,
    'a seed that named no mark described a region no mark encloses');

  assert.equal(areaFilled(rect, null), null, 'only a fill has a filled area');
  assert.equal(areaFilled({ kind: 'fill', shape: 'rect', a: { x: NaN, y: 0 }, b: { x: 1, y: 1 } }, null), null);
  assert.equal(areaFilled({ kind: 'fill', shape: 'free', points: [[0, 0], [1, 1]] }, null), null);
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

test('moving a shape moves the colour inside it exactly as far', () => {
  // The colour carries its own geometry, so it does not follow on its own. Writing the
  // fill back unmoved is what sent it home the instant the pointer came up, leaving the
  // outline where it was dropped.
  const ring = { points: [[100, 100], [200, 150], [100, 200]], color: '#000000', width: 6 };
  const paint = { kind: 'fill', shape: 'free', points: [[100, 100], [200, 150], [100, 200]],
    color: '#FFB703', anchor: 'ring' };
  let n = 0;
  const { groups, fresh } = moveEvents([['ring', ring], ['paint', paint]], ['ring'], 40, -25, () => `new-${n++}`);

  assert.equal(groups.length, 2, 'the shape and its colour both move');
  for (const group of groups) {
    assert.deepEqual(group.map(e => e.op), ['del', 'put'], 'a tombstone and its replacement');
    assert.equal(group[0].id !== group[1].id, true, 'an id is never reused');
  }
  const [outline, colour] = groups.map(group => group[1]);
  assert.deepEqual(outline.d.points, [[140, 75], [240, 125], [140, 175]]);
  assert.deepEqual(colour.d.points, outline.d.points, 'the same delta, not a different one');
  assert.equal(colour.d.anchor, fresh.get('ring'), 're-pointed at the shape it now belongs to');
  assert.equal(outline.d.layer, 'ring', 'painting order survives');
  assert.equal(colour.d.layer, 'paint');

  // A rectangle's colour is two corners rather than samples, and moves the same way.
  const box = { kind: 'rect', a: { x: 0, y: 0 }, b: { x: 100, y: 60 }, color: '#000000', width: 6 };
  const inside = { kind: 'fill', shape: 'rect', a: { x: 3, y: 3 }, b: { x: 97, y: 57 }, color: '#457B9D', anchor: 'box' };
  const boxed = moveEvents([['box', box], ['inside', inside]], ['box'], 10, 10, () => `new-${n++}`);
  const [, painted] = boxed.groups.map(group => group[1]);
  assert.deepEqual(painted.d.a, { x: 13, y: 13 });
  assert.deepEqual(painted.d.b, { x: 107, y: 67 });
});

test('a move touches nothing it was not given', () => {
  let n = 0;
  const mint = () => `new-${n++}`;
  const ring = { points: [[0, 0], [9, 9], [0, 18]], width: 6 };
  const other = { points: [[50, 50], [60, 60], [50, 70]], width: 6 };
  const paint = { kind: 'fill', shape: 'free', points: [[0, 0], [9, 9], [0, 18]], anchor: 'ring' };
  const loose = { kind: 'fill', shape: 'free', points: [[50, 50], [60, 60], [50, 70]], anchor: 'other' };
  const entries = [['ring', ring], ['other', other], ['paint', paint], ['loose', loose]];

  const { groups } = moveEvents(entries, ['ring'], 5, 5, mint);
  assert.deepEqual(groups.flat().filter(e => e.op === 'del').map(e => e.id), ['ring', 'paint'],
    'the colour on the other shape stays where it is');

  // A selection naming a mark that is gone writes nothing for it.
  assert.deepEqual(moveEvents(entries, ['vanished'], 5, 5, mint).groups, []);
  // And a fill whose anchor is not in the selection is not dragged along.
  assert.equal(moveEvents(entries, ['other'], 5, 5, mint).groups.length, 2);
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
test('every kind of mark reaches SVG, and a bare fill is reported rather than guessed at', () => {
  const { text: svg, skipped } = toSvg([
    ['a', freehand],
    ['b', { ...freehand, points: [[0, 0], [50, 50], [100, 0]] }],
    ['c', rect],
    ['d', { kind: 'line', a: { x: 0, y: 0 }, b: { x: 10, y: 10 }, color: '#E63946', width: 4, dash: 'dashed' }],
    ['e', { kind: 'ellipse', a: { x: 0, y: 0 }, b: { x: 40, y: 20 }, color: '#457B9D', width: 2 }],
    ['f', text],
    ['g', { kind: 'fill', x: 5, y: 5, color: '#FFB703' }]
  ]);
  assert.equal(skipped, 1, 'a seed from an older log that named no mark has no area');
  assert.match(svg, /^<svg xmlns="http:\/\/www\.w3\.org\/2000\/svg"/);
  assert.match(svg, new RegExp(`width="${SHEET_W}" height="${SHEET_H}"`));
  for (const element of ['<path ', '<rect ', '<line ', '<ellipse ', '<text ']) {
    assert.ok(svg.includes(element), `${element} is missing`);
  }
  assert.match(svg, /stroke-dasharray="12,8"/, 'the line style survives the export');
  assert.ok(!svg.includes('<svg', 1), 'one document, not a nest of them');
});

test('a translucent mark is exported as a group, which composites the way the canvas does', () => {
  const { text: svg } = toSvg([['a', { ...freehand, blend: 'multiply' }]]);
  assert.match(svg, /<g style="mix-blend-mode:multiply">/);
  assert.match(svg, /<\/g>/);
});

test('a pressure stroke keeps its per-point widths, and an even one becomes one curve', () => {
  const even = toSvg([['a', { points: [[0, 0], [10, 10], [20, 0], [30, 10]], color: '#000000', width: 6 }]]).text;
  assert.equal((even.match(/<path /g) || []).length, 1);
  assert.match(even, / Q /, 'curves through the midpoints, not a run of straight lines');

  const varying = toSvg([['a', { points: [[0, 0, 2], [10, 10, 9], [20, 0, 14]], color: '#000000', width: 6 }]]).text;
  assert.equal((varying.match(/<line /g) || []).length, 2, 'a segment per pair, each at its own width');
  assert.match(varying, /stroke-width="5.5"/);
});

test('text in a mark is escaped, never emitted as markup', () => {
  const { text: svg } = toSvg([['a', { ...text, text: '<script>alert(1)</script> & "more"' }]]);
  assert.ok(!svg.includes('<script>'), svg);
  assert.match(svg, /&lt;script&gt;alert\(1\)&lt;\/script&gt; &amp; /);
});

test('our own clipboard SVG is recognised by its stamp and nothing else is', () => {
  const stamp = 'sk' + (1234567890).toString(36);
  const { text: mine } = toSvg([['a', freehand]], stamp);
  assert.equal(isOurs(mine, stamp), true);
  assert.equal(isOurs(mine, 'sk-other'), false, 'a different session is a foreign payload');
  assert.equal(isOurs(toSvg([['a', freehand]]).text, stamp), false, 'an export carries no stamp');
  for (const empty of [null, '', undefined]) {
    assert.equal(isOurs(empty, stamp), false);
    assert.equal(isOurs(mine, empty), false);
  }
});

test('a fill is exported as the area it carries, whatever drew the outline', () => {
  const filledRect = { kind: 'fill', shape: 'rect', a: { x: 103, y: 103 }, b: { x: 297, y: 197 }, color: '#FFB703', anchor: 'r' };
  const { text: svg, skipped } = toSvg([['r', rect], ['paint', filledRect]]);
  assert.equal(skipped, 0);
  assert.match(svg, /<rect x="103" y="103" width="194" height="94" fill="#FFB703"\/>/);
  // The outline is still drawn, and the colour lands over it, as the log replays.
  assert.ok(svg.indexOf('stroke="#000000"') < svg.indexOf('fill="#FFB703"'), svg);

  const round = toSvg([['e', { kind: 'ellipse', a: { x: 0, y: 0 }, b: { x: 100, y: 60 }, color: '#000000', width: 4 }],
    ['paint', { kind: 'fill', shape: 'ellipse', a: { x: 2, y: 2 }, b: { x: 98, y: 58 }, color: '#457B9D', anchor: 'e' }]]);
  assert.match(round.text, /<ellipse cx="50" cy="30" rx="48" ry="28" fill="#457B9D"\/>/);

  // A circle drawn with the brush exported as an empty ring before fills had an area.
  const ring = { points: [[100, 50], [150, 100], [100, 150], [50, 100], [100, 50]], color: '#000000', width: 6 };
  const hand = toSvg([['ring', ring],
    ['paint', { kind: 'fill', shape: 'free', points: ring.points, color: '#FFB703', anchor: 'ring' }]]);
  assert.equal(hand.skipped, 0, 'a hand-drawn outline holds colour too');
  const area = hand.text.split('\n').find(line => line.includes('fill="#FFB703"'));
  assert.match(area, /^<path d="M 100 50 Q /, area);
  assert.match(area, / Z" fill="#FFB703" stroke="none"\/>$/, 'closed, and no outline of its own');
  const outline = hand.text.split('\n').find(line => line.includes('stroke="#000000"'));
  assert.ok(outline.includes('fill="none"') && !outline.includes(' Z"'),
    'the stroke itself is not closed behind the artist\'s back');
});

test('the sheet\'s own colour becomes the background, and is never a shape in the file', () => {
  const plain = toSvg([['a', freehand]]);
  assert.match(plain.text, /<rect width="1600" height="1200" fill="#FFFFFF"\/>/, 'white by default');
  const tinted = toSvg([['p', { kind: 'page', color: '#F6F1EE' }], ['a', freehand]], null, '#F6F1EE');
  assert.match(tinted.text, /<rect width="1600" height="1200" fill="#F6F1EE"\/>/);
  assert.equal(tinted.skipped, 0, 'the sheet is the surface, not something left out');
  assert.equal((tinted.text.match(/<rect /g) || []).length, 1, 'and it is written once');
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
  await m.change([put('paint', { kind: 'fill', shape: 'rect', a: { x: 3, y: 3 }, b: { x: 6, y: 6 }, color: '#FFB703', anchor: 'shape' })]);
  await m.change([{ op: 'del', tbl: 'stroke', id: 'shape' }]);
  await m.undo();
  const fresh = [...m.strokes.keys()].find(id => id !== 'paint');
  assert.notEqual(fresh, 'shape');
  assert.equal(m.strokes.get('paint').anchor, fresh,
    'or a later move of the shape would leave its colour behind');
});

// ---- a tab's own write, echoed back by the node --------------------------------------------
/**
 * The node publishes an append to the stream and answers the request that made it, in no
 * fixed order, so a tab's own write can arrive back before the call that wrote it returns.
 * A history that counts the echo as a change of its own remembers every mark twice, and
 * the second undo then refuses forever: the stack's next entry describes a mark that the
 * first undo already removed.
 */
function echoing({ early }) {
  let minted = 0;
  let clock = Date.parse('2026-09-07T12:00:00.000Z');
  let history;
  const written = [];
  history = new SketchHistory(async events => {
    const ts = new Date(clock).toISOString();
    clock += 40;
    const stamped = events.map(ev => ({ ...ev, ts, dev: 'k7m2q9xf' }));
    written.push(stamped);
    const echo = () => { for (const ev of stamped) history.apply(ev); };
    if (early) queueMicrotask(echo);           // the stream wins the race
    else setTimeout(echo, 0);                  // the response wins it
    return { appended: events.length };
  }, () => `fresh-${minted++}`);
  return { history, written };
}
const settle = () => new Promise(resolve => setTimeout(resolve, 0));

for (const early of [true, false]) {
  const when = early ? 'before the write returns' : 'after it';
  test(`a mark is remembered once when the stream echoes it ${when}`, async () => {
    const { history: m } = echoing({ early });
    for (let i = 0; i < 5; i++) {
      await m.change([put(`s${i}`)]);
      await settle();
    }
    assert.equal(m.strokes.size, 5);
    assert.equal(m.undoStack.length, 5, 'one entry per mark, not two');

    for (let left = 4; left >= 0; left--) {
      await m.undo();
      await settle();
      assert.equal(m.strokes.size, left, `undo down to ${left}`);
    }
    assert.equal(m.canUndo, false);
    assert.equal(m.strokes.size, 0, 'every mark undone, not just the last');
  });
}

test('the echo of a restoring write does not become an undo entry of its own', async () => {
  const { history: m } = echoing({ early: true });
  await m.change([put('shape')]);
  await settle();
  await m.change([{ op: 'del', tbl: 'stroke', id: 'shape' }]);
  await settle();

  await m.undo();                       // restores it under a fresh id
  await settle();
  assert.equal(m.strokes.size, 1);
  assert.equal(m.undoStack.length, 1, 'the compensating write is not a change of its own');

  await m.undo();                       // and the mark itself can still be taken back
  await settle();
  assert.equal(m.strokes.size, 0);
});

test('a change from another device is still remembered while our own write is in flight', async () => {
  const { history: m } = echoing({ early: true });
  const pending = m.change([put('mine')]);
  m.apply({ ...put('theirs'), ts: '2026-09-07T12:00:00.000Z', dev: 'b3nn8t2q' });
  await pending;
  await settle();
  assert.equal(m.strokes.size, 2);
  assert.equal(m.undoStack.length, 2, "the other device's mark is a change like any other");
});
