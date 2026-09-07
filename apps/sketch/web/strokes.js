/* Project: Privatium™ | File: apps/sketch/web/strokes.js
 * Authors: Gabriel Mongefranco (@gabrielmongefranco)
 * Created: 2026-09-06 | Modified: 2026-09-07
 * Summary: Geometry for the marks on the sheet: hit testing for selection and the stroke eraser,
          and bounds for the selection outline, the marquee and a fill's anchor. Independent
          of display scaling — everything here is in sheet coordinates. Changes no data.
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

/** The origin a fill anchors to: a shape's top-left corner in sheet coordinates. */
export function origin(mark) {
  if (!isShape(mark)) return null;
  return { x: Math.min(mark.a.x, mark.b.x), y: Math.min(mark.a.y, mark.b.y) };
}

/** A copy of a mark moved by a delta, including a fill's anchor reference. */
export function moved(mark, dx, dy) {
  const out = { ...mark };
  if (Array.isArray(mark.points)) out.points = mark.points.map(([x, y, w]) => (w === undefined ? [x + dx, y + dy] : [x + dx, y + dy, w]));
  if (mark.a && mark.b) { out.a = { x: mark.a.x + dx, y: mark.a.y + dy }; out.b = { x: mark.b.x + dx, y: mark.b.y + dy }; }
  if (Number.isFinite(mark.x) && Number.isFinite(mark.y)) { out.x = mark.x + dx; out.y = mark.y + dy; }
  if (mark.anchorAt) out.anchorAt = { x: mark.anchorAt.x + dx, y: mark.anchorAt.y + dy };
  return out;
}
