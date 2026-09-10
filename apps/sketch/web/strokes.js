// This file is part of Privatium
// apps/sketch/web/strokes.js
// Author(s): Gabriel Mongefranco
// Created: 2026-09-06
// Last Modified: 2026-09-07
// Summary: Geometry for the marks on the sheet: hit testing for selection and the stroke eraser, bounds
//          for the selection outline and the marquee, and the area a fill covers — a fill is
//          the inside of one mark, so it is geometry like everything else here rather than a
//          region found by searching pixels. Independent of display scaling — everything here
//          is in sheet coordinates. Changes no data.
// Notes: See README file for documentation and full license information.
//
// Copyright © 2026 Gabriel Mongefranco
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License along
// with this program. If not, see <https://www.gnu.org/licenses/>.

const SHAPES = ['line', 'rect', 'ellipse'];

function finite(...values) { return values.every(Number.isFinite); }

/** Whether a mark is one of the two-point shapes. A mark with no `kind` is freehand. */
export function isShape(mark) { return !!mark && SHAPES.includes(mark.kind); }

function validPoints(mark) {
  return Array.isArray(mark.points) && mark.points.length &&
    mark.points.every(p => Array.isArray(p) && p.length >= 2 && finite(p[0], p[1]));
}

/**
 * The mark's bounding box in sheet coordinates, padded by half its width, or null when
 * its geometry is unusable. Branches on what the mark IS rather than on which fields it
 * happens to carry — a shape holds `a` and `b`, freehand holds `points`.
 */
export function bounds(mark) {
  if (!mark || !Number.isFinite(mark.width) || mark.width <= 0) return null;
  const pad = mark.width / 2 + 8;
  if (isShape(mark)) {
    if (!mark.a || !mark.b || !finite(mark.a.x, mark.a.y, mark.b.x, mark.b.y)) return null;
    return [
      Math.min(mark.a.x, mark.b.x) - pad, Math.min(mark.a.y, mark.b.y) - pad,
      Math.max(mark.a.x, mark.b.x) + pad, Math.max(mark.a.y, mark.b.y) + pad
    ];
  }
  if (mark.kind === 'text') {
    if (!finite(mark.x, mark.y) || typeof mark.text !== 'string') return null;
    const size = textSize(mark);
    return [mark.x - 8, mark.y - size * 0.7, mark.x + mark.text.length * size * 0.62, mark.y + size * 0.7];
  }
  if (mark.kind === 'fill') return null;                 // a fill is a region, not an object
  if (!validPoints(mark)) return null;
  const xs = mark.points.map(p => p[0]);
  const ys = mark.points.map(p => p[1]);
  return [Math.min(...xs) - pad, Math.min(...ys) - pad, Math.max(...xs) + pad, Math.max(...ys) + pad];
}

/**
 * The mark's own geometry as a box, with no padding. `bounds` adds room for the width of
 * the stroke, which is right for an outline to grab by and wrong for arithmetic: the pad
 * is a constant, so scaling a padded box does not scale the mark inside it by the same
 * amount. Resizing measures with this.
 */
export function extent(mark) {
  if (!mark || mark.kind === 'fill' || mark.kind === 'page') return null;
  if (isShape(mark)) {
    if (!mark.a || !mark.b || !finite(mark.a.x, mark.a.y, mark.b.x, mark.b.y)) return null;
    return [
      Math.min(mark.a.x, mark.b.x), Math.min(mark.a.y, mark.b.y),
      Math.max(mark.a.x, mark.b.x), Math.max(mark.a.y, mark.b.y)
    ];
  }
  if (mark.kind === 'text') {
    const box = bounds(mark);
    return box ? [box[0] + 8, box[1], box[2], box[3]] : null;
  }
  if (!validPoints(mark)) return null;
  const xs = mark.points.map(p => p[0]);
  const ys = mark.points.map(p => p[1]);
  return [Math.min(...xs), Math.min(...ys), Math.max(...xs), Math.max(...ys)];
}

/** The box around several marks, or null when none of them has usable geometry. */
export function extentOf(marks) {
  let box = null;
  for (const mark of marks) {
    const one = extent(mark);
    if (!one) continue;
    box = box ? [Math.min(box[0], one[0]), Math.min(box[1], one[1]),
      Math.max(box[2], one[2]), Math.max(box[3], one[3])] : one.slice();
  }
  return box;
}

/** The type size a text mark is painted at, derived from its width so one control sets both. */
export function textSize(mark) { return Math.max(28, (mark.width || 6) * 5); }

/** Whether a point is within six sheet pixels of a freehand stroke's painted edge. */
export function hitsStroke(stroke, x, y) {
  if (!finite(x, y) || !stroke || !Number.isFinite(stroke.width) || stroke.width <= 0 ||
      !validPoints(stroke)) return false;
  const radius = stroke.width / 2 + 6;
  return stroke.points.some(([bx, by], i) => {
    const [ax, ay] = stroke.points[Math.max(0, i - 1)];
    const dx = bx - ax, dy = by - ay;
    const length = dx * dx + dy * dy;
    const t = length ? Math.max(0, Math.min(1, ((x - ax) * dx + (y - ay) * dy) / length)) : 0;
    return Math.hypot(x - ax - t * dx, y - ay - t * dy) <= radius;
  });
}

/**
 * Whether a point selects a mark. Freehand is tested against the painted path, so a
 * click inside a loose curve does not grab it; a shape or a text block is tested against
 * its box, which is what its selection outline shows and what people expect to grab.
 */
export function hits(mark, x, y) {
  if (!mark || mark.kind === 'fill') return false;
  if (isShape(mark) || mark.kind === 'text') {
    const b = bounds(mark);
    return !!b && x >= b[0] && x <= b[2] && y >= b[1] && y <= b[3];
  }
  return hitsStroke(mark, x, y);
}

/** Whether a mark's box intersects a marquee, given as two corners in sheet coordinates. */
export function inBox(mark, a, b) {
  const box = bounds(mark);
  if (!box) return false;
  const x0 = Math.min(a.x, b.x), x1 = Math.max(a.x, b.x);
  const y0 = Math.min(a.y, b.y), y1 = Math.max(a.y, b.y);
  return box[2] >= x0 && box[0] <= x1 && box[3] >= y0 && box[1] <= y1;
}

/** Whether a mark encloses an area, and so has an inside that can be coloured. */
export function encloses(mark) {
  if (!mark || mark.kind === 'fill' || mark.kind === 'page') return false;
  if (mark.kind === 'text' || mark.kind === 'line') return false;
  return isShape(mark) || validPoints(mark);
}

/** Whether a point is within a mark's box. What decides which shape a click is inside. */
export function covers(mark, x, y) {
  const box = bounds(mark);
  return !!box && finite(x, y) && x >= box[0] && x <= box[2] && y >= box[1] && y <= box[3];
}

/**
 * The inside of a mark, as the geometry a fill of it carries. A rectangle or an ellipse
 * is inset by half its outline, which is where the colour stops; a freehand outline is
 * its own samples, closed. Null when the mark encloses nothing, or encloses nothing left
 * once the outline is taken off it.
 */
export function areaOf(mark) {
  if (!encloses(mark)) return null;
  if (isShape(mark)) {
    const inset = (mark.width || 6) / 2;
    const x0 = Math.min(mark.a.x, mark.b.x) + inset, x1 = Math.max(mark.a.x, mark.b.x) - inset;
    const y0 = Math.min(mark.a.y, mark.b.y) + inset, y1 = Math.max(mark.a.y, mark.b.y) - inset;
    if (!(x1 > x0 && y1 > y0)) return null;
    return { shape: mark.kind, a: { x: x0, y: y0 }, b: { x: x1, y: y1 } };
  }
  if (mark.points.length < 3) return null;
  return { shape: 'free', points: mark.points.map(([x, y]) => [x, y]) };
}

/**
 * The area a fill covers.
 *
 * A fill written today carries its own geometry, so this is the fill itself and nothing
 * is looked up. A fill written before that — when a fill was a seed point flooded across
 * the pixels — is read as the inside of the mark it was anchored to, which is what it was
 * meant to be; one with no anchor described a region no single mark encloses and can no
 * longer be drawn.
 */
export function areaFilled(mark, host) {
  if (!mark || mark.kind !== 'fill') return null;
  if (mark.shape === 'rect' || mark.shape === 'ellipse') {
    if (!mark.a || !mark.b || !finite(mark.a.x, mark.a.y, mark.b.x, mark.b.y)) return null;
    return { shape: mark.shape, a: mark.a, b: mark.b };
  }
  if (mark.shape === 'free') {
    return validPoints(mark) && mark.points.length >= 3
      ? { shape: 'free', points: mark.points }
      : null;
  }
  return host ? areaOf(host) : null;
}

/**
 * A copy of a mark scaled about a fixed point.
 *
 * The outline keeps its thickness: `width` is the pen a mark was drawn with, so a scaled
 * shape stays drawn with the same pen, and a pressure stroke keeps the width it recorded
 * at every sample. Text is the exception — its `width` is the type size rather than a
 * thickness — and it scales uniformly by the smaller factor, since one size cannot follow
 * two axes.
 */
export function scaled(mark, ax, ay, sx, sy) {
  const out = { ...mark };
  const px = x => ax + (x - ax) * sx;
  const py = y => ay + (y - ay) * sy;
  if (Array.isArray(mark.points)) {
    out.points = mark.points.map(([x, y, w]) =>
      (w === undefined ? [px(x), py(y)] : [px(x), py(y), w]));
  }
  if (mark.a && mark.b) {
    out.a = { x: px(mark.a.x), y: py(mark.a.y) };
    out.b = { x: px(mark.b.x), y: py(mark.b.y) };
  }
  if (finite(mark.x, mark.y)) { out.x = px(mark.x); out.y = py(mark.y); }
  if (mark.kind === 'text' && Number.isFinite(mark.width)) {
    out.width = Math.max(1, mark.width * Math.min(sx, sy));
  }
  return out;
}

/**
 * The events that rewrite a selection through a transform.
 *
 * Each mark becomes a tombstone and a fresh put, since an id is never reused, carrying its
 * original layer so painting order survives. A fill that belongs to one of the marks
 * travels in the same batch, through the **same** transform — it holds its own geometry,
 * so it does not follow on its own — and is re-pointed at its mark's new id. Moving and
 * resizing both come through here so a fill can never be given a transform of its own.
 *
 * Returns the events in groups. A group is written whole, so a tombstone and the put that
 * replaces it are never split across two batches.
 */
export function rewriteEvents(entries, ids, transform, mint) {
  const byId = new Map(entries);
  const chosen = new Set(ids);
  const fresh = new Map();
  for (const id of ids) if (byId.has(id)) fresh.set(id, mint());

  const rewrite = (id, mark) => {
    const d = { ...transform(mark), layer: mark.layer || id };
    if (d.kind === 'fill' && fresh.has(d.anchor)) d.anchor = fresh.get(d.anchor);
    return d;
  };
  const group = (id, mark) => [
    { op: 'del', tbl: 'stroke', id },
    { op: 'put', tbl: 'stroke', id: fresh.get(id), d: rewrite(id, mark) }
  ];

  const groups = [];
  for (const id of ids) if (byId.has(id)) groups.push(group(id, byId.get(id)));
  for (const [id, mark] of entries) {
    if (fresh.has(id) || mark.kind !== 'fill' || !chosen.has(mark.anchor)) continue;
    fresh.set(id, mint());
    groups.push(group(id, mark));
  }
  return { groups, fresh };
}

/** The events that move a selection by a delta. */
export function moveEvents(entries, ids, dx, dy, mint) {
  return rewriteEvents(entries, ids, mark => moved(mark, dx, dy), mint);
}

/** The events that scale a selection about a fixed point. */
export function resizeEvents(entries, ids, ax, ay, sx, sy, mint) {
  return rewriteEvents(entries, ids, mark => scaled(mark, ax, ay, sx, sy), mint);
}

/** The smallest a selection may be scaled to, in sheet pixels, so it cannot vanish. */
export const MIN_EXTENT = 8;

/**
 * The scale that puts a box's far corner under a point, clamped so nothing collapses or
 * turns inside out. An axis with no extent — a horizontal line has no height — cannot be
 * scaled at all and is left alone rather than dividing by zero. `uniform` keeps the
 * proportions, taking the smaller factor so the shape stays within the pointer.
 */
export function scaleTo(box, x, y, uniform) {
  const w = box[2] - box[0], h = box[3] - box[1];
  let sx = w > 0 ? Math.max(MIN_EXTENT / w, (x - box[0]) / w) : 1;
  let sy = h > 0 ? Math.max(MIN_EXTENT / h, (y - box[1]) / h) : 1;
  if (uniform && w > 0 && h > 0) sx = sy = Math.min(sx, sy);
  return { sx, sy };
}

/** A copy of a mark moved by a delta. Fields this file does not understand — an old
 *  fill's `anchorAt` among them — are carried over untouched (`spec/protocol.md §4.2`). */
export function moved(mark, dx, dy) {
  const out = { ...mark };
  if (Array.isArray(mark.points)) out.points = mark.points.map(([x, y, w]) => (w === undefined ? [x + dx, y + dy] : [x + dx, y + dy, w]));
  if (mark.a && mark.b) { out.a = { x: mark.a.x + dx, y: mark.a.y + dy }; out.b = { x: mark.b.x + dx, y: mark.b.y + dy }; }
  if (Number.isFinite(mark.x) && Number.isFinite(mark.y)) { out.x = mark.x + dx; out.y = mark.y + dy; }
  return out;
}
