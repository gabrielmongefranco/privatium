// This file is part of Privatium
// apps/pantry/web/views.js
// Author(s): Gabriel Mongefranco
// Created: 2026-09-07
// Last Modified: 2026-09-08
// Summary: Everything this app puts on the screen, built with createElement and textContent. No
//          innerHTML anywhere: markup built from a value is the injection a Content Security
//          Policy cannot see (spec/app-contract.md §5.4). Amounts arrive from the data API as
//          strings and are shown as strings — a DECIMAL is text on purpose, and turning one
//          into a JavaScript number is the bug the framework's exact arithmetic exists to
//          prevent. Every state is a word beside its icon, never a colour on its own, and
//          every control that acts on one row names that row, so a screen reader hears which
//          batch it is about.
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

/** The icons a batch may carry — the fixed set the add form offers, because an arbitrary
 * name is not in the page's sprite and would render an empty box (PV503). */
export const ICONS = [
  'snow', 'cup-hot', 'egg-fried', 'basket', 'box-seam', 'droplet', 'cookie', 'bag',
];

/** Every symbol the page's sprite defines: the batch icons and the ones controls use. */
const SPRITE = ICONS.concat([
  'plus-lg', 'box-arrow-right', 'box-arrow-in-left', 'arrow-down-up', 'clock-history',
  'exclamation-triangle', 'calendar-x', 'calendar-event', 'check2', 'cloud-slash',
  'arrow-repeat', 'list-ul', 'caret-down-fill',
]);

/**
 * One element.
 * @param {string} tag The element name.
 * @param {object} [props] Attributes; `text` sets textContent, a null value is skipped.
 * @param {Array<Node|string>} [children] Children to append, in order.
 * @returns {HTMLElement} The element.
 */
export function el(tag, props = {}, children = []) {
  const node = document.createElement(tag);
  for (const [name, value] of Object.entries(props)) {
    if (value === null || value === undefined) continue;
    if (name === 'text') node.textContent = value;
    else node.setAttribute(name, String(value));
  }
  for (const child of children) node.append(child);
  return node;
}

/**
 * One inline icon from the page's sprite (docs/icons.md), cloned from the `<template>` in
 * the page rather than built here — an SVG element has to be created in its own namespace,
 * and the page already holds one shaped correctly. Every icon this app draws sits beside
 * text, so every one of them is decorative and carries `aria-hidden`.
 * @param {string} name The Bootstrap Icons name; one the sprite lacks falls back to the box.
 * @returns {SVGElement} The `<svg>`, always `focusable="false"`.
 */
export function icon(name) {
  const svg = document.getElementById('icon-shape').content.firstElementChild.cloneNode(true);
  svg.firstElementChild.setAttribute('href', `#i-${SPRITE.includes(name) ? name : 'box-seam'}`);
  return svg;
}

/**
 * A stored decimal without its trailing zeros, for reading. The value stays a string.
 * @param {string} amount The value as the API returned it, such as `"2.500"`.
 * @returns {string} The same number, shorter: `"2.5"`.
 */
export function trim(amount) {
  if (typeof amount !== 'string' || !amount.includes('.')) return amount == null ? '' : String(amount);
  return amount.replace(/0+$/, '').replace(/\.$/, '');
}

/**
 * Whether a stored decimal is zero, decided on the text rather than on a number.
 * @param {string} amount The value as the API returned it.
 * @returns {boolean} True when every digit is zero.
 */
export function isZero(amount) {
  return !/[1-9]/.test(String(amount == null ? '' : amount));
}

/** Words for what a batch's date says, so nothing depends on colour (PV405). */
const EXPIRY = { past: 'Expired', soon: 'Use soon', fresh: '', unknown: '' };

/**
 * A timestamp from the log, in the reader's own time zone and short enough for a column.
 * @param {string} at The column's RFC 3339 UTC value.
 * @returns {string} A local date and time, or the raw value if it cannot be read.
 */
export function localTime(at) {
  const when = new Date(at);
  if (Number.isNaN(when.getTime())) return String(at);
  return when.toLocaleString(undefined, {
    day: 'numeric', month: 'short', hour: '2-digit', minute: '2-digit',
  });
}

/**
 * The unit as it reads beside one particular amount. Units are written plural — bags,
 * containers — so exactly one of something drops the s.
 * @param {string} amount The amount as text, trimmed.
 * @param {string} unit The batch's unit.
 * @returns {string} The unit, singular when the amount is one.
 */
function units(amount, unit) {
  const one = amount === '1' || amount === '-1';
  return one && unit.endsWith('s') ? unit.slice(0, -1) : unit;
}

/** An amount and its unit: the number large, the unit quiet beneath it. */
function quantity(amount, unit) {
  const shown = trim(amount);
  return el('td', { class: 'right qty' }, [
    el('span', { text: shown }),
    el('span', { class: 'unit', text: units(shown, unit) }),
  ]);
}

/**
 * The shelf map: one slat per shelf, the open one marked.
 * @param {HTMLElement} into The container to fill.
 * @param {Array<object>} shelves Rows of `v_shelf`.
 * @param {string} open The id of the open shelf.
 * @param {Function} choose Called with a shelf id when one is chosen.
 */
export function renderShelves(into, shelves, open, choose) {
  into.replaceChildren();
  for (const shelf of shelves) {
    const held = shelf.batches === 1 ? '1 batch' : `${shelf.batches} batches`;
    const button = el('button', {
      type: 'button',
      class: 'slat',
      'aria-pressed': shelf.id === open ? 'true' : 'false',
    }, [
      el('span', { text: shelf.name }),
      el('span', { class: 'count', text: held }),
    ]);
    button.addEventListener('click', () => choose(shelf.id));
    into.append(button);
  }
}

/** The panel a row opens: take some out, or move the batch. */
function rowPanel(row, shelves, on) {
  const takeId = `take-${row.id}`;
  const moveId = `move-${row.id}`;
  const errId = `err-${row.id}`;
  const error = el('p', { class: 'err', id: errId, hidden: 'hidden' });

  const takeInput = el('input', {
    id: takeId, type: 'text', inputmode: 'decimal', 'aria-describedby': errId,
  });
  const takeForm = el('form', { class: 'inline' }, [
    el('div', { class: 'field' }, [
      el('label', { for: takeId, text: `Take out (${row.unit})` }),
      takeInput,
    ]),
    el('button', { type: 'submit', class: 'go', text: 'Take out' }),
  ]);
  takeForm.addEventListener('submit', event => {
    event.preventDefault();
    on.take(row, takeInput, error);
  });

  const select = el('select', { id: moveId });
  for (const shelf of shelves) {
    const option = el('option', { value: shelf.id, text: shelf.name });
    if (shelf.id === row.shelf_id) option.setAttribute('selected', 'selected');
    select.append(option);
  }
  const moveForm = el('form', { class: 'inline' }, [
    el('div', { class: 'field' }, [
      el('label', { for: moveId, text: 'Move to' }),
      select,
    ]),
    el('button', { type: 'submit', text: 'Move' }),
  ]);
  moveForm.addEventListener('submit', event => {
    event.preventDefault();
    on.move(row, select.value, error);
  });

  const panel = el('div', { class: 'panel', hidden: 'hidden' }, [takeForm, moveForm, error]);
  return { panel, takeInput };
}

/**
 * The batch table for the open shelf.
 * @param {HTMLElement} into The `<tbody>` to fill.
 * @param {Array<object>} rows Rows of `v_batch`.
 * @param {Array<object>} shelves Rows of `v_shelf`, for the move control.
 * @param {object} on Handlers: `take(row, input, error)` and `move(row, shelfId, error)`.
 */
export function renderBatches(into, rows, shelves, on) {
  into.replaceChildren();
  for (const row of rows) {
    const flag = EXPIRY[row.expiry] || '';
    const useBy = el('td', { class: 'stored' });
    if (flag) {
      useBy.append(el('span', { class: 'flag' }, [
        icon(row.expiry === 'past' ? 'calendar-x' : 'calendar-event'),
        el('span', { text: flag }),
      ]), ' ');
    }
    useBy.append(el('span', { text: row.expires_on || 'no date' }));

    const { panel, takeInput } = rowPanel(row, shelves, on);
    const open = el('button', {
      type: 'button',
      class: 'act',
      'aria-expanded': 'false',
      'aria-label': `Take out or move ${row.name}`,
    }, [
      icon('box-arrow-right'),
      el('span', { text: 'Take out' }),
    ]);
    open.addEventListener('click', () => {
      const shown = panel.hasAttribute('hidden');
      panel.toggleAttribute('hidden', !shown);
      open.setAttribute('aria-expanded', shown ? 'true' : 'false');
      if (shown) takeInput.focus();
    });

    into.append(el('tr', {}, [
      el('th', { scope: 'row' }, [
        el('span', { class: 'item' }, [icon(row.icon), el('span', { text: row.name })]),
      ]),
      quantity(row.balance, row.unit),
      el('td', { class: 'stored stored-col', text: row.stored_on }),
      el('td', { class: 'right days', text: `${row.days_in}` }),
      useBy,
      el('td', {}, [el('div', { class: 'acts' }, [open]), panel]),
    ]));
  }
}

/**
 * The tray: one card per withdrawal that has not fully come back.
 * @param {HTMLElement} into The list to fill.
 * @param {Array<object>} rows Rows of `v_out`.
 * @param {Function} back Called as `back(row, input, error)` when a return is submitted.
 * @returns {number} How many cards were rendered.
 */
export function renderTray(into, rows, back) {
  into.replaceChildren();
  let shown = 0;
  for (const row of rows) {
    if (isZero(row.still_out)) continue;
    shown += 1;
    const fieldId = `back-${row.id}`;
    const errId = `back-err-${row.id}`;
    const error = el('p', { class: 'err', id: errId, hidden: 'hidden' });
    const input = el('input', {
      id: fieldId, type: 'text', inputmode: 'decimal', 'aria-describedby': errId,
    });
    const form = el('form', { class: 'inline' }, [
      el('div', { class: 'field' }, [
        el('label', { for: fieldId, text: `Put back (${row.unit})` }),
        input,
      ]),
      el('button', { type: 'submit', class: 'go' }, [
        icon('box-arrow-in-left'),
        el('span', { text: 'Put back' }),
      ]),
    ]);
    form.addEventListener('submit', event => {
      event.preventDefault();
      back(row, input, error);
    });

    const still = isZero(row.returned)
      ? `${trim(row.still_out)} ${row.unit} out`
      : `${trim(row.still_out)} of ${trim(row.taken)} ${row.unit} still out`;

    into.append(el('li', { class: 'card-out' }, [
      el('p', { class: 'out-head' }, [
        icon(row.icon),
        el('span', { text: row.batch_name }),
        el('span', { class: 'count', text: still }),
      ]),
      el('p', { class: 'out-when', text: `${row.shelf_name} · ${localTime(row.at)}` }),
      form,
      error,
    ]));
  }
  return shown;
}

/** What one activity entry says it was. */
const REASON = { stocked: 'Stocked', taken: 'Took out', returned: 'Put back' };

/**
 * The activity list, read from the log rather than from a table.
 * @param {HTMLElement} into The `<tbody>` to fill.
 * @param {Array<object>} entries `{ id, d, undone }`, newest first.
 * @param {Map<string, object>} batches Batch id to `{ name, unit }` for the batches still known.
 * @param {Function} undo Called with a change id when Undo is pressed.
 */
export function renderActivity(into, entries, batches, undo) {
  into.replaceChildren();
  for (const entry of entries) {
    const batch = batches.get(entry.d.batch_id);
    const name = batch ? batch.name : 'a batch that is no longer listed';
    const what = `${REASON[entry.d.reason] || entry.d.reason} ${name}`;
    const shown = trim(entry.d.amount);
    const amount = batch ? `${shown} ${units(shown, batch.unit)}` : shown;
    const action = el('td', { class: 'right' });
    if (entry.undone) {
      action.append(el('span', { class: 'undone', text: 'undone' }));
    } else {
      const button = el('button', {
        type: 'button',
        class: 'quiet',
        'aria-label': `Undo: ${what}`,
      }, [el('span', { text: 'Undo' })]);
      button.addEventListener('click', () => undo(entry.id));
      action.append(button);
    }
    into.append(el('tr', { class: entry.undone ? 'is-undone' : null }, [
      el('th', { scope: 'row', class: 'when', text: localTime(entry.d.at) }),
      el('td', { text: what }),
      el('td', { class: 'right', text: amount }),
      action,
    ]));
  }
}

/**
 * The two check lists: a balance below zero, and a withdrawal more than fully returned.
 * @param {HTMLElement} into The list to fill.
 * @param {Array<object>} batches Rows of `v_check_batch`.
 * @param {Array<object>} outs Rows of `v_check_out`.
 * @returns {number} How many entries were rendered.
 */
export function renderChecks(into, batches, outs) {
  into.replaceChildren();
  for (const row of batches) {
    into.append(el('li', { class: 'check' }, [
      el('strong', { text: `${row.name}: ${trim(row.balance)} ${row.unit}` }),
      el('p', {
        text: `On ${row.shelf_name}. Two devices took the same last portion, so the log `
          + 'adds up to less than nothing. Both takings are kept and neither was clamped: '
          + 'put back what is really there, or record what you found.',
      }),
    ]));
  }
  for (const row of outs) {
    into.append(el('li', { class: 'check' }, [
      el('strong', {
        text: `${row.batch_name}: ${trim(row.returned)} back of ${trim(row.taken)} ${row.unit}`,
      }),
      el('p', {
        text: 'More came back than went out. Both returns are recorded; neither was dropped.',
      }),
    ]));
  }
  return batches.length + outs.length;
}

/**
 * The expiring table.
 * @param {HTMLElement} into The `<tbody>` to fill.
 * @param {Array<object>} rows Rows of `v_expiring`.
 */
export function renderExpiring(into, rows) {
  into.replaceChildren();
  for (const row of rows) {
    const useBy = el('td', { class: 'stored' }, [
      el('span', { class: 'flag' }, [
        icon(row.expiry === 'past' ? 'calendar-x' : 'calendar-event'),
        el('span', { text: row.expiry === 'past' ? 'Expired' : 'Use soon' }),
      ]),
      ' ',
      el('span', { text: row.expires_on }),
    ]);
    into.append(el('tr', {}, [
      el('th', { scope: 'row' }, [
        el('span', { class: 'item' }, [icon(row.icon), el('span', { text: row.name })]),
      ]),
      el('td', { text: row.shelf_name }),
      useBy,
      quantity(row.balance, row.unit),
    ]));
  }
}

/**
 * The stock summary, one row per unit of measure.
 * @param {HTMLElement} into The `<tbody>` to fill.
 * @param {Array<object>} rows Rows of `v_stock_by_unit`.
 */
export function renderSummary(into, rows) {
  into.replaceChildren();
  for (const row of rows) {
    into.append(el('tr', {}, [
      el('th', { scope: 'row', text: row.unit }),
      el('td', { class: 'right', text: trim(row.balance) }),
      el('td', { class: 'right', text: `${row.batches}` }),
    ]));
  }
}
