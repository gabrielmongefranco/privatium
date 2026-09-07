/* Project: Privatium™ | File: apps/sketch/web/paint.js
 * Authors: Gabriel Mongefranco (@gabrielmongefranco)
 * Created: 2026-09-07 | Modified: 2026-09-07
 * Summary: Painting one mark, and the area of a fill. Everything here draws in sheet
          coordinates; the device scale arrives as k for the one operation that is
          unavoidably in pixels, compositing a translucent stroke, which is painted whole
          off-screen and composited once so it does not darken against its own overlapping
          segments. A fill is a path, not a search: it is the inside of one mark, so it
          costs a fill() rather than a read and write of the whole surface.
 *          See main README.md for full license information.
 */
let scratch = null;

/** The dash pattern for a line style, scaled by the stroke width so it reads the same
 *  at Fine and at Broad. */
export function dashFor(width, kind) {
  if (kind === 'dashed') return [width * 3, width * 2];
  if (kind === 'dotted') return [1, width * 2.2];
  return [];
}

/** Paint a mark in sheet coordinates. Fills are not painted here — see paintArea. */
export function paintMark(ctx, mark, k) {
  if (mark.kind === 'fill' || mark.kind === 'page') return;
  if (mark.blend === 'multiply' && mark.kind !== 'eraser' && mark.color !== '#FFFFFF') {
    paintBlended(ctx, mark, k);
    return;
  }
  stroke(ctx, mark);
}

function stroke(ctx, mark) {
  const width = mark.width || 6;
  ctx.strokeStyle = mark.color;
  ctx.fillStyle = mark.color;
  ctx.lineWidth = width;
  ctx.setLineDash(mark.dash ? dashFor(width, mark.dash) : []);

  if (mark.kind === 'text') {
    const size = Math.max(28, width * 5);
    ctx.font = '600 ' + size + 'px system-ui, sans-serif';
    ctx.textBaseline = 'middle';
    ctx.fillText(mark.text, mark.x, mark.y);
    ctx.setLineDash([]);
    return;
  }
  if (mark.kind === 'rect') {
    ctx.strokeRect(Math.min(mark.a.x, mark.b.x), Math.min(mark.a.y, mark.b.y),
      Math.abs(mark.b.x - mark.a.x), Math.abs(mark.b.y - mark.a.y));
    ctx.setLineDash([]);
    return;
  }
  if (mark.kind === 'ellipse') {
    ctx.beginPath();
    ctx.ellipse((mark.a.x + mark.b.x) / 2, (mark.a.y + mark.b.y) / 2,
      Math.abs(mark.b.x - mark.a.x) / 2, Math.abs(mark.b.y - mark.a.y) / 2, 0, 0, Math.PI * 2);
    ctx.stroke();
    ctx.setLineDash([]);
    return;
  }
  if (mark.kind === 'line') {
    ctx.beginPath();
    ctx.moveTo(mark.a.x, mark.a.y);
    ctx.lineTo(mark.b.x, mark.b.y);
    ctx.stroke();
    ctx.setLineDash([]);
    return;
  }

  ctx.setLineDash([]);
  const points = mark.points;
  if (!points || !points.length) return;
  if (points.length === 1) {
    ctx.beginPath();
    ctx.arc(points[0][0], points[0][1], width / 2, 0, Math.PI * 2);
    ctx.fill();
    return;
  }
  if (points.length === 2) {
    ctx.lineWidth = points[1][2] || width;
    ctx.beginPath();
    ctx.moveTo(points[0][0], points[0][1]);
    ctx.lineTo(points[1][0], points[1][1]);
    ctx.stroke();
    return;
  }
  // Curves through the sample midpoints, not a run of straight lines: the same samples
  // drawn as segments are what made a slowly drawn circle look faceted.
  const base = points[0][2] || width;
  const even = points.every(p => (p[2] || width) === base);
  if (even) {
    ctx.lineWidth = base;
    ctx.beginPath();
    trace(ctx, points);
    ctx.stroke();
    return;
  }
  // A pressure stroke varies in width, so it is painted a segment at a time, each one a
  // curve through its own control point at its own width.
  for (let i = 1; i < points.length; i++) {
    const a = points[i - 1], b = points[i];
    ctx.lineWidth = ((a[2] || width) + (b[2] || width)) / 2;
    const from = i === 1 ? a : [(points[i - 2][0] + a[0]) / 2, (points[i - 2][1] + a[1]) / 2];
    const to = i === points.length - 1 ? b : [(a[0] + b[0]) / 2, (a[1] + b[1]) / 2];
    ctx.beginPath();
    ctx.moveTo(from[0], from[1]);
    ctx.quadraticCurveTo(a[0], a[1], to[0], to[1]);
    ctx.stroke();
  }
}

/** The curve a freehand mark follows: quadratics through the sample midpoints. The
 *  outline and the colour inside it are the same path, so they cannot drift apart. */
function trace(ctx, points) {
  ctx.moveTo(points[0][0], points[0][1]);
  for (let i = 1; i < points.length - 1; i++) {
    ctx.quadraticCurveTo(points[i][0], points[i][1],
      (points[i][0] + points[i + 1][0]) / 2, (points[i][1] + points[i + 1][1]) / 2);
  }
  ctx.lineTo(points[points.length - 1][0], points[points.length - 1][1]);
}

/** Colour the inside of one mark, given the area from `strokes.areaFilled`. */
export function paintArea(ctx, area, colour) {
  if (!area) return;
  ctx.save();
  ctx.fillStyle = colour;
  ctx.beginPath();
  if (area.shape === 'free') {
    trace(ctx, area.points);
    ctx.closePath();
  } else if (area.shape === 'rect') {
    ctx.rect(area.a.x, area.a.y, area.b.x - area.a.x, area.b.y - area.a.y);
  } else {
    ctx.ellipse((area.a.x + area.b.x) / 2, (area.a.y + area.b.y) / 2,
      (area.b.x - area.a.x) / 2, (area.b.y - area.a.y) / 2, 0, 0, Math.PI * 2);
  }
  ctx.fill();
  ctx.restore();
}

function paintBlended(ctx, mark, k) {
  const w = ctx.canvas.width, h = ctx.canvas.height;
  if (!scratch) scratch = document.createElement('canvas');
  if (scratch.width !== w || scratch.height !== h) { scratch.width = w; scratch.height = h; }
  const off = scratch.getContext('2d');
  off.setTransform(1, 0, 0, 1, 0, 0);
  off.clearRect(0, 0, w, h);
  off.setTransform(k, 0, 0, k, 0, 0);
  off.lineCap = 'round';
  off.lineJoin = 'round';
  stroke(off, mark);
  ctx.save();
  ctx.setTransform(1, 0, 0, 1, 0, 0);
  ctx.globalCompositeOperation = 'multiply';
  ctx.drawImage(scratch, 0, 0);
  ctx.restore();
}
