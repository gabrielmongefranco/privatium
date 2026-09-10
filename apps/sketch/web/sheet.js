// This file is part of Privatium
// apps/sketch/web/sheet.js
// Author(s): Gabriel Mongefranco
// Created: 2026-09-07
// Last Modified: 2026-09-07
// Summary: The sheet is a fixed 1600 x 1200 coordinate space, so a stroke drawn on one device lands on
//          the same pixels on every other. That is separate from the canvas's pixel size: the
//          CSS box follows fit or zoom, the backing store follows the CSS box at
//          devicePixelRatio, and the ratio between them rides in the context transform.
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

export const SHEET_W = 1600;
export const SHEET_H = 1200;

const ZOOM_KEY = 'sketch.zoom';
const MIN_ZOOM = 0.05;
const MAX_ZOOM = 4;

/**
 * Owns the mapping between the sheet's coordinates and the pixels on screen.
 * Nothing else in the app needs to know about devicePixelRatio or the zoom level.
 */
export class Sheet {
  constructor(canvas, viewport) {
    this.canvas = canvas;
    this.viewport = viewport;
    // View state, not app data: it belongs to this device and must never reach the log.
    const stored = parseFloat(localStorage.getItem(ZOOM_KEY) || '0');
    this.zoom = stored >= MIN_ZOOM && stored <= MAX_ZOOM ? stored : 0;   // 0 means fit
    this.k = 1;
  }

  /** The scale the sheet is shown at: an explicit zoom, or whatever fits the viewport. */
  get scale() { return this.zoom > 0 ? this.zoom : this.fitScale(); }

  fitScale() {
    const room = 28;
    const w = Math.max(120, this.viewport.clientWidth - room);
    const h = Math.max(120, this.viewport.clientHeight - room);
    return Math.min(w / SHEET_W, h / SHEET_H);
  }

  /**
   * Size the element in CSS and its backing store to that box at the device's pixel
   * ratio — never from innerWidth, which draws past the viewport on every HiDPI display,
   * and never below the sheet's own resolution. Returns true when the store was resized,
   * which clears it: the caller has to repaint.
   */
  resize() {
    const s = this.scale;
    const cssW = Math.round(SHEET_W * s);
    const cssH = Math.round(SHEET_H * s);
    this.canvas.style.width = cssW + 'px';
    this.canvas.style.height = cssH + 'px';
    const ratio = Math.min(3, window.devicePixelRatio || 1);
    const bw = Math.max(SHEET_W, Math.round(cssW * ratio));
    const bh = Math.max(SHEET_H, Math.round(cssH * ratio));
    let resized = false;
    if (this.canvas.width !== bw || this.canvas.height !== bh) {
      this.canvas.width = bw;
      this.canvas.height = bh;
      resized = true;
    }
    this.k = bw / SHEET_W;
    return resized;
  }

  /** Put the context in sheet coordinates. Setting canvas.width resets it, so this is
   *  called outright after every resize rather than multiplied into an existing scale. */
  applyTo(ctx) {
    ctx.setTransform(this.k, 0, 0, this.k, 0, 0);
    ctx.lineCap = 'round';
    ctx.lineJoin = 'round';
  }

  /** A pointer event in sheet coordinates. Mapped through the element's own box, so it
   *  is correct at any zoom and identical on every device. */
  at(event) {
    const r = this.canvas.getBoundingClientRect();
    return {
      x: (event.clientX - r.left) * SHEET_W / r.width,
      y: (event.clientY - r.top) * SHEET_H / r.height
    };
  }

  /** Sheet coordinates to backing-store pixels, for the pixel work: sampling and fill. */
  toDevice(p) {
    return { x: Math.round(p.x * this.k), y: Math.round(p.y * this.k) };
  }

  setZoom(z) {
    this.zoom = z > 0 ? Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, z)) : 0;
    if (this.zoom > 0) localStorage.setItem(ZOOM_KEY, String(this.zoom));
    else localStorage.removeItem(ZOOM_KEY);
  }

  /** Buttons step in whole 5 % increments from wherever the view is now. */
  step(direction) {
    const now = Math.round(this.scale * 20) * 5;
    this.setZoom((now + direction * 5) / 100);
  }

  get percent() { return Math.round(this.scale * 100); }

  /** True when the sheet is larger than its viewport, so panning has somewhere to go. */
  get pannable() {
    return this.viewport.scrollWidth > this.viewport.clientWidth + 1 ||
           this.viewport.scrollHeight > this.viewport.clientHeight + 1;
  }

  panBy(dx, dy) {
    this.viewport.scrollLeft += dx;
    this.viewport.scrollTop += dy;
  }
}
