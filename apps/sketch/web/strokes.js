/* Project: Privatium™ | File: apps/sketch/web/strokes.js
 * Authors: Gabriel Mongefranco (@gabrielmongefranco)
 * Created: 2026-09-06 | Modified: 2026-09-06
 * Summary: Hit testing for the whole-stroke eraser, independent of display scaling.
 */

/** Return whether a canvas point is within six pixels of a stroke's painted edge.
 * Invalid stroke geometry or coordinates return false; this function changes no data.
 */
export function hitsStroke(stroke, x, y) {
  if (!Number.isFinite(x) || !Number.isFinite(y) || !stroke ||
      !Number.isFinite(stroke.width) || stroke.width <= 0 ||
      !Array.isArray(stroke.points) || !stroke.points.length ||
      !stroke.points.every(p => Array.isArray(p) && p.length === 2 && p.every(Number.isFinite))) return false;
  const radius = stroke.width / 2 + 6;
  return stroke.points.some(([bx, by], i) => {
    const [ax, ay] = stroke.points[Math.max(0, i - 1)];
    const dx = bx - ax, dy = by - ay;
    const length = dx * dx + dy * dy;
    const t = length ? Math.max(0, Math.min(1, ((x - ax) * dx + (y - ay) * dy) / length)) : 0;
    return Math.hypot(x - ax - t * dx, y - ay - t * dy) <= radius;
  });
}
