/* Project: Privatium™ | File: apps/sketch/web/clip.js
 * Authors: Gabriel Mongefranco (@gabrielmongefranco)
 * Created: 2026-09-07 | Modified: 2026-09-07
 * Summary: Copy, cut and paste over marks, and the SVG the clipboard carries. Our own clipboard
          SVG is stamped, so a paste can tell it from a foreign one and use the real marks
          rather than a re-read of their outlines. The SVG reader is deliberately small:
          straight geometry only, and it says what it left behind.
 *          See main README.md for full license information.
 */
import { SHEET_W, SHEET_H } from './sheet.js';
import { bounds, moved, textSize } from './strokes.js';
import { dashFor } from './paint.js';

const STAMP = 'data-sketch-clip';

const round = v => Math.round(v * 10) / 10;
const escape = text => String(text).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');

/** The curve a freehand mark traces: quadratics through the sample midpoints, the same
 *  path the canvas draws. `close` shuts it, which is what a fill inside it needs. */
function freehandPath(points, close) {
  let d = 'M ' + round(points[0][0]) + ' ' + round(points[0][1]);
  for (let i = 1; i < points.length - 1; i++) {
    d += ' Q ' + round(points[i][0]) + ' ' + round(points[i][1]) + ' ' +
      round((points[i][0] + points[i + 1][0]) / 2) + ' ' + round((points[i][1] + points[i + 1][1]) / 2);
  }
  d += ' L ' + round(points[points.length - 1][0]) + ' ' + round(points[points.length - 1][1]);
  return close ? d + ' Z' : d;
}

/**
 * A fill that belongs to a mark, as that mark's own area.
 *
 * For a rectangle or an ellipse the flood stops at the inside edge of the outline, so the
 * filled copy is inset by half the stroke width. For a freehand outline — a circle drawn
 * by hand — it is the same closed curve the stroke traces, filled and unstroked, which is
 * as close as a path can come to what the flood covered.
 */
function filledArea(host, colour) {
  if (host.kind === 'rect' || host.kind === 'ellipse') {
    const inset = (host.width || 6) / 2;
    const x0 = Math.min(host.a.x, host.b.x), x1 = Math.max(host.a.x, host.b.x);
    const y0 = Math.min(host.a.y, host.b.y), y1 = Math.max(host.a.y, host.b.y);
    const w = Math.max(0, x1 - x0 - inset * 2), h = Math.max(0, y1 - y0 - inset * 2);
    if (!w || !h) return null;
    if (host.kind === 'rect') {
      return '<rect x="' + round(x0 + inset) + '" y="' + round(y0 + inset) +
        '" width="' + round(w) + '" height="' + round(h) + '" fill="' + colour + '"/>';
    }
    return '<ellipse cx="' + round((x0 + x1) / 2) + '" cy="' + round((y0 + y1) / 2) +
      '" rx="' + round(w / 2) + '" ry="' + round(h / 2) + '" fill="' + colour + '"/>';
  }
  if (host.kind || !Array.isArray(host.points) || host.points.length < 3) return null;
  return '<path d="' + freehandPath(host.points, true) + '" fill="' + colour + '" stroke="none"/>';
}

/**
 * Serialise marks as SVG, given as [id, mark] pairs in painting order. Freehand becomes
 * one path of quadratic segments — the same curve the canvas draws — and a translucent
 * mark is wrapped in a group, which composites as a unit so overlaps inside it stay
 * normal. A fill anchored to a mark is written as that mark's own area, which is what the
 * flood actually covers; a fill on the open page is a region of pixels with no outline
 * behind it and cannot be expressed, so it is counted and left out.
 */
export function toSvg(entries, stamp) {
  const out = ['<svg xmlns="http://www.w3.org/2000/svg"' + (stamp ? ' ' + STAMP + '="' + stamp + '"' : '') +
    ' width="' + SHEET_W + '" height="' + SHEET_H + '" viewBox="0 0 ' + SHEET_W + ' ' + SHEET_H + '">',
    '<rect width="' + SHEET_W + '" height="' + SHEET_H + '" fill="#FFFFFF"/>'];
  let skipped = 0;
  const byId = new Map(entries);

  for (const [, mark] of entries) {
    if (mark.kind === 'fill') {
      const host = mark.anchor ? byId.get(mark.anchor) : null;
      const area = host ? filledArea(host, mark.color) : null;
      if (area) out.push(area);
      else skipped++;
      continue;
    }
    const width = mark.width || 6;
    const dash = mark.dash
      ? ' stroke-dasharray="' + dashFor(width, mark.dash).map(round).join(',') + '"'
      : '';
    const body = [];

    if (mark.kind === 'text') {
      body.push('<text x="' + round(mark.x) + '" y="' + round(mark.y) + '" fill="' + mark.color +
        '" font-family="system-ui, sans-serif" font-weight="600" font-size="' + textSize(mark) +
        '" dominant-baseline="middle">' + escape(mark.text) + '</text>');
    } else if (mark.kind === 'line') {
      body.push('<line x1="' + round(mark.a.x) + '" y1="' + round(mark.a.y) + '" x2="' + round(mark.b.x) +
        '" y2="' + round(mark.b.y) + '" stroke="' + mark.color + '" stroke-width="' + width +
        '" stroke-linecap="round"' + dash + '/>');
    } else if (mark.kind === 'rect') {
      body.push('<rect x="' + round(Math.min(mark.a.x, mark.b.x)) + '" y="' + round(Math.min(mark.a.y, mark.b.y)) +
        '" width="' + round(Math.abs(mark.b.x - mark.a.x)) + '" height="' + round(Math.abs(mark.b.y - mark.a.y)) +
        '" fill="none" stroke="' + mark.color + '" stroke-width="' + width + '"' + dash + '/>');
    } else if (mark.kind === 'ellipse') {
      body.push('<ellipse cx="' + round((mark.a.x + mark.b.x) / 2) + '" cy="' + round((mark.a.y + mark.b.y) / 2) +
        '" rx="' + round(Math.abs(mark.b.x - mark.a.x) / 2) + '" ry="' + round(Math.abs(mark.b.y - mark.a.y) / 2) +
        '" fill="none" stroke="' + mark.color + '" stroke-width="' + width + '"' + dash + '/>');
    } else {
      const points = mark.points || [];
      if (points.length < 2) continue;
      const base = points[0][2] || width;
      if (points.every(p => (p[2] || width) === base)) {
        const d = freehandPath(points, false);
        body.push('<path d="' + d + '" fill="none" stroke="' + mark.color + '" stroke-width="' + round(base) +
          '" stroke-linecap="round" stroke-linejoin="round"/>');
      } else {
        for (let i = 1; i < points.length; i++)
          body.push('<line x1="' + round(points[i - 1][0]) + '" y1="' + round(points[i - 1][1]) +
            '" x2="' + round(points[i][0]) + '" y2="' + round(points[i][1]) + '" stroke="' + mark.color +
            '" stroke-width="' + round(((points[i - 1][2] || width) + (points[i][2] || width)) / 2) +
            '" stroke-linecap="round"/>');
      }
    }

    if (mark.blend === 'multiply') out.push('<g style="mix-blend-mode:multiply">', ...body, '</g>');
    else out.push(...body);
  }

  out.push('</svg>');
  return { text: out.join('\n'), skipped };
}

/** Whether clipboard text is the SVG this app wrote for the given stamp. */
export function isOurs(text, stamp) {
  return !!stamp && !!text && text.includes(STAMP + '="' + stamp + '"');
}

/**
 * Read marks out of foreign SVG: straight lines, rectangles, ellipses, circles,
 * polylines, polygons, and paths using only M, L and Z. Curves, text, images, groups with
 * transforms and everything else are counted and skipped rather than guessed at. Geometry
 * that would land off the sheet is translated onto it.
 */
export function fromSvg(text, fallbackColor, fallbackWidth) {
  let doc;
  try { doc = new DOMParser().parseFromString(text, 'image/svg+xml'); } catch { return null; }
  if (!doc || doc.querySelector('parsererror') || !doc.querySelector('svg')) return null;

  const numbers = value => String(value || '').trim().split(/[\s,]+/).map(Number).filter(Number.isFinite);
  const at = (el, name) => { const n = parseFloat(el.getAttribute(name)); return Number.isFinite(n) ? n : 0; };
  const colorOf = el => {
    const value = (el.getAttribute('stroke') || '').trim();
    return /^#[0-9a-f]{6}$/i.test(value) ? value.toUpperCase() : fallbackColor;
  };
  const widthOf = el => {
    const value = parseFloat(el.getAttribute('stroke-width'));
    return Number.isFinite(value) && value > 0 ? value : fallbackWidth;
  };

  const marks = [];
  let skipped = 0;

  for (const el of doc.querySelector('svg').querySelectorAll('*')) {
    const tag = el.tagName.toLowerCase();
    if (tag === 'line') {
      marks.push({ kind: 'line', a: { x: at(el, 'x1'), y: at(el, 'y1') }, b: { x: at(el, 'x2'), y: at(el, 'y2') },
        color: colorOf(el), width: widthOf(el) });
    } else if (tag === 'rect') {
      marks.push({ kind: 'rect', a: { x: at(el, 'x'), y: at(el, 'y') },
        b: { x: at(el, 'x') + at(el, 'width'), y: at(el, 'y') + at(el, 'height') },
        color: colorOf(el), width: widthOf(el) });
    } else if (tag === 'ellipse' || tag === 'circle') {
      const rx = tag === 'circle' ? at(el, 'r') : at(el, 'rx');
      const ry = tag === 'circle' ? at(el, 'r') : at(el, 'ry');
      marks.push({ kind: 'ellipse', a: { x: at(el, 'cx') - rx, y: at(el, 'cy') - ry },
        b: { x: at(el, 'cx') + rx, y: at(el, 'cy') + ry }, color: colorOf(el), width: widthOf(el) });
    } else if (tag === 'polyline' || tag === 'polygon') {
      const values = numbers(el.getAttribute('points'));
      const points = [];
      for (let i = 0; i + 1 < values.length; i += 2) points.push([values[i], values[i + 1]]);
      if (tag === 'polygon' && points.length > 2) points.push([points[0][0], points[0][1]]);
      if (points.length > 1) marks.push({ points, color: colorOf(el), width: widthOf(el) });
    } else if (tag === 'path') {
      const d = el.getAttribute('d') || '';
      if (/[csqtaCSQTA]/.test(d)) { skipped++; continue; }
      const values = numbers(d.replace(/[MmLlZz]/g, ' '));
      const points = [];
      for (let i = 0; i + 1 < values.length; i += 2) points.push([values[i], values[i + 1]]);
      if (points.length > 1) marks.push({ points, color: colorOf(el), width: widthOf(el) });
      else skipped++;
    } else if (['text', 'image', 'use', 'tspan', 'clipPath', 'mask', 'filter', 'pattern'].includes(tag)) {
      skipped++;
    }
  }

  if (!marks.length) return null;

  let x0 = Infinity, y0 = Infinity;
  for (const mark of marks) {
    const box = bounds(mark);
    if (!box) continue;
    x0 = Math.min(x0, box[0]);
    y0 = Math.min(y0, box[1]);
  }
  const dx = (x0 < 0 || x0 > SHEET_W - 40) ? 80 - x0 : 0;
  const dy = (y0 < 0 || y0 > SHEET_H - 40) ? 80 - y0 : 0;
  return { marks: (dx || dy) ? marks.map(m => moved(m, dx, dy)) : marks, skipped };
}
