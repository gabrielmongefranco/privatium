// This file is part of Privatium
// apps/sketch/web/tools.js
// Author(s): Gabriel Mongefranco
// Created: 2026-09-07
// Last Modified: 2026-09-07
// Summary: The tool set, the sizes, the line styles, the named inks and the default color row — the
//          tables the toolbars are built from. Also the contrast arithmetic the interface
//          leans on: a swatch is only self-evident when its own fill clears 3:1 against the
//          panel, so anything below that is given a boundary that does.
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

export const TOOLS = [
  { id: 'select', label: 'Select',     key: 'V', icon: 'cursor' },
  { id: 'brush',  label: 'Brush',      key: 'B', icon: 'brush' },
  { id: 'eraser', label: 'Eraser',     key: 'E', icon: 'eraser' },
  { id: 'pick',   label: 'Eyedropper', key: 'I', icon: 'eyedropper' },
  { id: 'fill',   label: 'Fill',       key: 'F', icon: 'paint-bucket' },
  { id: 'line',   label: 'Line',       key: 'L', icon: 'slash' },
  { id: 'rect',   label: 'Rectangle',  key: 'R', icon: 'square' },
  { id: 'ellipse',label: 'Ellipse',    key: 'O', icon: 'circle' },
  { id: 'text',   label: 'Text',       key: 'T', icon: 'type' },
  { id: 'pan',    label: 'Pan',        key: 'H', icon: 'arrows-move' }
];

/** The tools always on the top bar. The fourth slot follows whatever else was last used. */
export const BASIC = ['select', 'brush', 'eraser'];

/** Tools that lay ink down, and so have a color, a width, and something to eyedrop for. */
export const INKING = ['brush', 'fill', 'line', 'rect', 'ellipse', 'text'];
export const WIDTHED = ['brush', 'eraser', 'line', 'rect', 'ellipse', 'text'];
export const DASHED = ['line', 'rect', 'ellipse'];

export const SIZES = [
  { px: 2,  label: 'Fine',   dot: 6 },
  { px: 6,  label: 'Medium', dot: 10 },
  { px: 14, label: 'Bold',   dot: 15 },
  { px: 32, label: 'Broad',  dot: 22 }
];

export const DASHES = [
  { id: 'solid',  label: 'Solid' },
  { id: 'dashed', label: 'Dashed' },
  { id: 'dotted', label: 'Dotted' }
];

/** The color row starts here and returns here on a new sheet. Every slot is editable. */
export const DEFAULT_SLOTS = ['#000000', '#E63946', '#457B9D', '#FFB703', '', '', ''];

/** Two rows of paint-named inks in the custom color dialog: warm above, cool below. */
export const INKS = [
  { hex: '#E23D28', name: 'Warm red, cadmium' },
  { hex: '#F5A300', name: 'Warm yellow, deep' },
  { hex: '#7CB518', name: 'Warm green, grass' },
  { hex: '#2E4C9E', name: 'Warm blue, ultramarine' },
  { hex: '#8A4B2A', name: 'Warm brown, burnt sienna' },
  { hex: '#FFFFFF', name: 'White' },
  { hex: '#D6006E', name: 'Cool red, magenta' },
  { hex: '#F2E63D', name: 'Cool yellow, lemon' },
  { hex: '#00795B', name: 'Cool green, phthalo' },
  { hex: '#00A6D6', name: 'Cool blue, cyan' },
  { hex: '#000000', name: 'Black' },
  { hex: '#808080', name: 'Mid gray' }
];

const NAMED = {};
for (const ink of INKS) NAMED[ink.hex] = ink.name;
NAMED['#000000'] = 'Black';
NAMED['#E63946'] = 'Red';
NAMED['#457B9D'] = 'Blue';
NAMED['#FFB703'] = 'Yellow';

/** A color's name where the app has one, and its hex where it does not. Never "colour 3". */
export function colorName(hex) {
  if (!hex) return 'empty';
  return NAMED[hex.toUpperCase()] || hex.toUpperCase();
}

export function isHex(value) { return /^#[0-9a-f]{6}$/i.test(String(value || '').trim()); }

/** WCAG relative luminance. The sRGB channels are linearised: a plain channel average
 *  lets #FFB703 through as though it were dark, and it is not. */
export function luminance(hex) {
  const n = parseInt(hex.slice(1), 16);
  const channel = value => {
    const v = value / 255;
    return v <= 0.04045 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4);
  };
  return 0.2126 * channel(n >> 16 & 255) + 0.7152 * channel(n >> 8 & 255) + 0.0722 * channel(n & 255);
}

export function contrast(a, b) {
  const l1 = luminance(a), l2 = luminance(b);
  return (Math.max(l1, l2) + 0.05) / (Math.min(l1, l2) + 0.05);
}

/** Black or white, whichever reads better on the given fill. */
export function inkOn(hex) {
  return contrast(hex, '#FFFFFF') >= contrast(hex, '#000000') ? '#FFFFFF' : '#000000';
}

/**
 * Whether a swatch needs a drawn boundary: true when its own fill is under 3:1 against
 * the panel behind it (WCAG 1.4.11). Yellow on the light panel and black on the dark one
 * are the cases this catches; the hairline border never satisfies it on its own.
 */
export function needsEdge(hex, panel) {
  return !hex || contrast(hex, panel) < 3;
}

/** The panel color as a hex string, read from the stylesheet so it follows the scheme. */
export function panelColor() {
  const raw = getComputedStyle(document.documentElement).getPropertyValue('--panel').trim();
  if (isHex(raw)) return raw.toUpperCase();
  const rgb = raw.match(/rgba?\(\s*(\d+)[,\s]+(\d+)[,\s]+(\d+)/i);
  if (rgb) return '#' + [1, 2, 3].map(i => Number(rgb[i]).toString(16).padStart(2, '0')).join('').toUpperCase();
  return '#FFFCFA';
}
