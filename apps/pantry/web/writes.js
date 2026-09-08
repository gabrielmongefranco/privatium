/*
 * Project:  Privatium™  |  File: apps/pantry/web/writes.js
 * Authors:  Gabriel Mongefranco (@gabrielmongefranco)
 * Created:  2026-09-07  |  Modified: 2026-09-07
 * Summary:  Every change this app can make to the log, one function each, and every one of
 *           them exactly one pv.append of one batch. Adding a batch writes the batch row
 *           and its first quantity_change together, because a batch that exists with no
 *           stock would have no balance at all: decimal_sum() over no rows is NULL. A move
 *           is a put on the batch's own id, never a tombstone and a new one — a new id
 *           would orphan every change that points at the old one (spec/protocol.md §4.5,
 *           §4.6). Amounts are strings from the field to the log and are never numbers.
 *           See main README.md for full license information.
 */
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
