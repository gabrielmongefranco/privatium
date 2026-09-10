// This file is part of Privatium
// apps/pantry/web/writes.js
// Author(s): Gabriel Mongefranco
// Created: 2026-09-07
// Last Modified: 2026-09-07
// Summary: Every change this app can make to the log, one function each, and every one of them exactly
//          one pv.append of one batch. Adding a batch writes the batch row and its first
//          quantity_change together, because a batch that exists with no stock would have no
//          balance at all: decimal_sum() over no rows is NULL. A move is a put on the batch's
//          own id, never a tombstone and a new one — a new id would orphan every change that
//          points at the old one (spec/protocol.md §4.5, §4.6). Amounts are strings from the
//          field to the log and are never numbers.
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

import { pv } from '/static/pv.js';

/** This device's clock, in UTC, as the `at` column of a change. The envelope's own `ts`
 * is the node's and stays the node's (spec/data-api.md §2).
 * @returns {string} RFC 3339 UTC.
 */
export function stamp() {
  return new Date().toISOString();
}

/**
 * Add a shelf.
 * @param {string} name The shelf's name, as typed.
 * @param {number} sort Where it sits in the map.
 * @returns {Promise<object>} The append's response, or `{ queued: true }` when offline.
 */
export function addShelf(name, sort) {
  return pv.append([{ op: 'put', tbl: 'shelf', id: pv.ulid(), d: { name, sort } }]);
}

/**
 * Add a batch and the stock it arrives with, in one batch of the log.
 * @param {object} batch The batch row: name, icon, unit, shelf_id, stored_on, expires_on.
 * @param {string} amount How much arrived, as text at any scale up to the column's three.
 * @returns {Promise<object>} The append's response, or `{ queued: true }` when offline.
 */
export function addBatch(batch, amount) {
  const id = pv.ulid();
  return pv.append([
    { op: 'put', tbl: 'batch', id, d: batch },
    {
      op: 'put',
      tbl: 'quantity_change',
      id: pv.ulid(),
      d: { batch_id: id, amount, reason: 'stocked', of_id: null, at: stamp() },
    },
  ]);
}

/**
 * Move a batch to another shelf: the whole row again, under the id it already has.
 * @param {string} id The batch's id.
 * @param {object} batch The batch row as it now is, carrying the new `shelf_id`.
 * @returns {Promise<object>} The append's response, or `{ queued: true }` when offline.
 */
export function moveBatch(id, batch) {
  return pv.append([{ op: 'put', tbl: 'batch', id, d: batch }]);
}

/**
 * Take some out. The change is negative; nothing checks it against the balance, because
 * another device may be writing its own withdrawal at the same moment.
 * @param {string} batchId The batch taken from.
 * @param {string} amount How much, positive, as text.
 * @returns {Promise<object>} The append's response, or `{ queued: true }` when offline.
 */
export function takeOut(batchId, amount) {
  return pv.append([{
    op: 'put',
    tbl: 'quantity_change',
    id: pv.ulid(),
    d: { batch_id: batchId, amount: `-${amount}`, reason: 'taken', of_id: null, at: stamp() },
  }]);
}

/**
 * Put some of a withdrawal back. The return points at the withdrawal it undoes part of,
 * which is what lets a view say how much of it is still out.
 * @param {object} out One row of `v_out`: the withdrawal and its batch.
 * @param {string} amount How much comes back, positive, as text.
 * @returns {Promise<object>} The append's response, or `{ queued: true }` when offline.
 */
export function putBack(out, amount) {
  return pv.append([{
    op: 'put',
    tbl: 'quantity_change',
    id: pv.ulid(),
    d: { batch_id: out.batch_id, amount, reason: 'returned', of_id: out.id, at: stamp() },
  }]);
}

/**
 * Undo one recorded change: a tombstone on that row. Two devices undoing the same change
 * write the same tombstone, and the merge rule takes the last of them, so undoing twice
 * and undoing once leave the same state.
 * @param {string} id The change's id.
 * @returns {Promise<object>} The append's response, or `{ queued: true }` when offline.
 */
export function undoChange(id) {
  return pv.append([{ op: 'del', tbl: 'quantity_change', id }]);
}
