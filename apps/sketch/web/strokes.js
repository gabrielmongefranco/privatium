/* Project: Privatium™ | File: apps/sketch/web/strokes.js
 * Authors: Gabriel Mongefranco (@gabrielmongefranco)
 * Created: 2026-09-06 | Modified: 2026-09-07
 * Summary: Geometry for the marks on the sheet: hit testing for selection and the stroke eraser,
          bounds for the selection outline and the marquee, and the area a fill covers — a
          fill is the inside of one mark, so it is geometry like everything else here rather
          than a region found by searching pixels. Independent of display scaling —
          everything here is in sheet coordinates. Changes no data.
 *          See main README.md for full license information.
 */
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
 * The events that move a selection.
 *
 * Each mark becomes a tombstone and a fresh put, since an id is never reused, carrying its
 * original layer so painting order survives. A fill that belongs to one of the moved marks
 * travels in the same batch: it holds its own geometry, so it has to be moved by the same
 * delta — it does not follow on its own — and re-pointed at its mark's new id.
 *
 * Returns the events in groups. A group is written whole, so a tombstone and the put that
 * replaces it are never split across two batches.
 */
export function moveEvents(entries, ids, dx, dy, mint) {
  const byId = new Map(entries);
  const chosen = new Set(ids);
  const fresh = new Map();
  for (const id of ids) if (byId.has(id)) fresh.set(id, mint());

  const relocate = (id, mark) => {
    const d = { ...moved(mark, dx, dy), layer: mark.layer || id };
    if (d.kind === 'fill' && fresh.has(d.anchor)) d.anchor = fresh.get(d.anchor);
    return d;
  };
  const group = (id, mark) => [
    { op: 'del', tbl: 'stroke', id },
    { op: 'put', tbl: 'stroke', id: fresh.get(id), d: relocate(id, mark) }
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

/** A copy of a mark moved by a delta. Fields this file does not understand — an old
 *  fill's `anchorAt` among them — are carried over untouched (`spec/protocol.md §4.2`). */
export function moved(mark, dx, dy) {
  const out = { ...mark };
  if (Array.isArray(mark.points)) out.points = mark.points.map(([x, y, w]) => (w === undefined ? [x + dx, y + dy] : [x + dx, y + dy, w]));
  if (mark.a && mark.b) { out.a = { x: mark.a.x + dx, y: mark.a.y + dy }; out.b = { x: mark.b.x + dx, y: mark.b.y + dy }; }
  if (Number.isFinite(mark.x) && Number.isFinite(mark.y)) { out.x = mark.x + dx; out.y = mark.y + dy; }
  return out;
}
