/* Project: Privatium™ | File: apps/sketch/web/paint.js
 * Authors: Gabriel Mongefranco (@gabrielmongefranco)
 * Created: 2026-09-07 | Modified: 2026-09-07
 * Summary: Painting one mark, and the flood fill. Everything here draws in sheet coordinates;
          the device scale arrives as k for the two operations that are unavoidably in
          pixels — the flood, and compositing a translucent stroke. A translucent stroke is
          painted whole off-screen and composited once, so it does not darken against its
          own overlapping segments.
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

/** Paint a mark in sheet coordinates. Fills are not painted here — see floodMark. */
export function paintMark(ctx, mark, k) {
  if (mark.kind === 'fill') return;
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
    ctx.moveTo(points[0][0], points[0][1]);
    for (let i = 1; i < points.length - 1; i++)
      ctx.quadraticCurveTo(points[i][0], points[i][1],
        (points[i][0] + points[i + 1][0]) / 2, (points[i][1] + points[i + 1][1]) / 2);
    ctx.lineTo(points[points.length - 1][0], points[points.length - 1][1]);
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

/**
 * Flood from a seed in sheet coordinates. The seed and the scan are in backing-store
 * pixels, since that is where the pixels are; tolerance is 28 per channel, which keeps
 * an anti-aliased edge from leaking.
 *
 * Returns the region it covered, in sheet coordinates, so a caller can tell what the
 * fill actually landed inside; null when it covered nothing.
 */
export function floodMark(ctx, seed, hex, k) {
  const W = ctx.canvas.width, H = ctx.canvas.height;
  const x = Math.max(0, Math.min(W - 1, Math.round(seed.x * k)));
  const y = Math.max(0, Math.min(H - 1, Math.round(seed.y * k)));
  const image = ctx.getImageData(0, 0, W, H);
  const data = image.data;
  const start = (y * W + x) * 4;
  const target = [data[start], data[start + 1], data[start + 2]];
  const n = parseInt(hex.slice(1), 16);
  const r = n >> 16 & 255, g = n >> 8 & 255, b = n & 255;
  if (target[0] === r && target[1] === g && target[2] === b) return null;
  const stack = [y * W + x];
  const seen = new Uint8Array(W * H);
  let x0 = W, y0 = H, x1 = -1, y1 = -1;
  while (stack.length) {
    const at = stack.pop();
    if (seen[at]) continue;
    seen[at] = 1;
    const i = at * 4;
    if (Math.abs(data[i] - target[0]) > 28 || Math.abs(data[i + 1] - target[1]) > 28 ||
        Math.abs(data[i + 2] - target[2]) > 28) continue;
    data[i] = r; data[i + 1] = g; data[i + 2] = b; data[i + 3] = 255;
    const px = at % W, py = (at - px) / W;
    if (px < x0) x0 = px;
    if (py < y0) y0 = py;
    if (px > x1) x1 = px;
    if (py > y1) y1 = py;
    if (px > 0) stack.push(at - 1);
    if (px < W - 1) stack.push(at + 1);
    if (py > 0) stack.push(at - W);
    if (py < H - 1) stack.push(at + W);
  }
  ctx.putImageData(image, 0, 0);
  if (x1 < 0) return null;
  return { x0: x0 / k, y0: y0 / k, x1: x1 / k, y1: y1 / k };
}
