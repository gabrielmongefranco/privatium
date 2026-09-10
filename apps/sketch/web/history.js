// This file is part of Privatium
// apps/sketch/web/history.js
// Author(s): Gabriel Mongefranco
// Created: 2026-09-06
// Last Modified: 2026-09-07
// Summary: Undo and redo over the shared canvas: every change the tab sees — its own, another window's,
//          another device's, and the log replayed at load — joins one bounded history in the
//          order it arrived, and undo reverses the latest with compensating events. Original
//          stroke order survives restoration, and because a tombstoned id is never reused,
//          anything restored is written under a fresh one and every reference to it follows.
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

/** The node refuses a batch over this and writes none of it (`spec/data-api.md §3`). */
export const MAX_BATCH = 1000;

/**
 * Pack groups of events into batches no larger than the node accepts. A group is written
 * whole, so a mark's tombstone and the fresh put that replaces it are never split across
 * two writes; each returned batch is then one act, and so one undo step.
 */
export function batches(groups, max = MAX_BATCH) {
  const out = [];
  let current = [];
  for (const group of groups) {
    if (current.length && current.length + group.length > max) {
      out.push(current);
      current = [];
    }
    current = current.concat(group);
  }
  if (current.length) out.push(current);
  return out;
}

/** Retain up to 50 changes to the canvas. The supplied writer durably appends events. */
export class SketchHistory {
  constructor(write, mint) {
    this.write = write;
    this.mint = mint;
    this.strokes = new Map();
    this.order = new Map();
    this.undoStack = [];
    this.redoStack = [];
    this.busy = false;
    // The events of the write currently in flight. The node publishes an append to the
    // stream and answers the request that made it, in no fixed order, so a tab's own
    // write can arrive back before the call that wrote it has returned. Counting that
    // echo as a change of its own remembers every mark twice and leaves the second undo
    // refusing, because by then the stack's next entry describes a mark that is gone.
    this.inflight = new Set();
  }
  /** A row and the operation on it: what makes an echo recognisable as our own. */
  static key(ev) { return `${ev.tbl}/${ev.id}/${ev.op}`; }
  /**
   * Apply an event from replay, subscription or a successful local write, and remember
   * how to reverse it. An event that changes nothing — this tab's own write echoed back
   * by the stream, a replayed line already applied — is not remembered. Events that
   * share one `ts` and one `dev` were written as one batch and are undone as one.
   */
  apply(ev, remember = true) {
    if (this.inflight.has(SketchHistory.key(ev))) remember = false;
    const before = this.strokes.has(ev.id) ? this.strokes.get(ev.id) : undefined;
    if (ev.op === 'del') {
      if (before === undefined) return;
      this.strokes.delete(ev.id);
    } else {
      if (before !== undefined && JSON.stringify(before) === JSON.stringify(ev.d)) return;
      const layer = ev.d.layer || ev.id;
      if (!this.order.has(layer)) this.order.set(layer, this.order.size);
      this.strokes.set(ev.id, ev.d);
    }
    if (!remember) return;
    // A change from anywhere invalidates the redo branch, exactly as a new local one does.
    this.redoStack.length = 0;
    const inverse = before === undefined
      ? {op:'del', tbl:'stroke', id:ev.id}
      : {op:'put', tbl:'stroke', id:ev.id, d:before};
    const applied = {op:ev.op, tbl:'stroke', id:ev.id, ...(ev.op === 'del' ? {} : {d:ev.d})};
    const batch = ev.ts && ev.dev ? `${ev.dev}/${ev.ts}` : null;
    const top = this.undoStack.at(-1);
    if (batch && top && top.batch === batch) {
      top.events.push(applied);
      top.inverse.unshift(inverse);
    } else {
      this.remember({events:[applied], inverse:[inverse], batch});
    }
  }
  /** Read visible strokes in their original painting order, including restored strokes. */
  entries() {
    return [...this.strokes].sort(([a, x], [b, y]) =>
      this.order.get(x.layer || a) - this.order.get(y.layer || b));
  }
  remember(entry) {
    this.undoStack.push({events:entry.events.slice(), inverse:entry.inverse.slice(), batch:entry.batch});
    if (this.undoStack.length > 50) this.undoStack.shift();
  }
  get canUndo() { return this.undoStack.length > 0; }
  get canRedo() { return this.redoStack.length > 0; }
  /** Append an action from this tab and remember its inverse only after success; reject concurrent writes. */
  async change(events) {
    if (this.busy) throw new Error('Wait for the current action to finish.');
    if (!events.length) return;
    const inverse = events.map(ev => this.strokes.has(ev.id)
      ? {op:'put', tbl:'stroke', id:ev.id, d:this.strokes.get(ev.id)}
      : {op:'del', tbl:'stroke', id:ev.id});
    this.busy = true;
    try {
      return await this.writing(events, async () => {
        const result = await this.write(events);
        events.forEach(ev => this.apply(ev, false));
        this.redoStack.length = 0;
        this.remember({events, inverse, batch:null});
        return result;
      });
    } finally { this.busy = false; }
  }
  /** Run a write with its events marked as ours, so the stream's echo of them is applied
   *  but not remembered, whichever of the two arrives first. */
  async writing(events, run) {
    const keys = events.map(ev => SketchHistory.key(ev));
    for (const key of keys) this.inflight.add(key);
    try { return await run(); } finally { for (const key of keys) this.inflight.delete(key); }
  }
  /**
   * Write one direction of a remembered action. A put whose row is currently a tombstone
   * cannot reuse its id (`spec/protocol.md §4.6`; the API answers 409), so it is written
   * under a fresh one carrying the original layer, and every reference held elsewhere —
   * the other entries in both stacks, and any fill anchored to the mark — is re-pointed
   * through the returned map.
   */
  async compensate(list) {
    const restored = new Map();
    const events = list.map(ev => {
      if (ev.op !== 'put' || this.strokes.has(ev.id)) return ev;
      const replacement = {...ev, id:this.mint(), d:{...ev.d, layer:ev.d.layer || ev.id}};
      restored.set(ev.id, replacement);
      return replacement;
    });
    const result = await this.writing(events, async () => {
      const out = await this.write(events);
      events.forEach(ev => this.apply(ev, false));
      return out;
    });
    if (restored.size) {
      for (const entry of [...this.undoStack, ...this.redoStack]) {
        for (const list of [entry.events, entry.inverse]) {
          list.forEach((ev, at) => {
            const replacement = restored.get(ev.id);
            if (!replacement) return;
            list[at] = ev.op === 'put'
              ? {...ev, id:replacement.id, d:{...ev.d, layer:replacement.d.layer}}
              : {...ev, id:replacement.id};
          });
        }
      }
      // A fill anchored to a restored shape must follow it, or it silently stops drawing.
      for (const [id, mark] of this.strokes) {
        if (mark.kind !== 'fill' || !mark.anchor) continue;
        const replacement = restored.get(mark.anchor);
        if (replacement) this.strokes.set(id, {...mark, anchor:replacement.id});
      }
    }
    return { result, restored };
  }
  /** Undo the latest change to the canvas, wherever it came from, without overwriting a later change to its strokes. */
  async undo() {
    if (this.busy) throw new Error('Wait for the current action to finish.');
    const action = this.undoStack.at(-1);
    if (!action) return;
    const matches = action.events.every(ev => ev.op === 'del'
      ? !this.strokes.has(ev.id)
      : JSON.stringify(this.strokes.get(ev.id)) === JSON.stringify(ev.d));
    if (!matches) throw new Error('These strokes changed in another window. Undo is unavailable.');
    this.busy = true;
    try {
      const { result } = await this.compensate(action.inverse);
      this.undoStack.pop();
      this.redoStack.push(action);
      return result;
    } finally { this.busy = false; }
  }
  /** Put back the change undo just reversed, under fresh ids where it has to. */
  async redo() {
    if (this.busy) throw new Error('Wait for the current action to finish.');
    const action = this.redoStack.at(-1);
    if (!action) return;
    this.busy = true;
    try {
      const { result } = await this.compensate(action.events);
      this.redoStack.pop();
      this.undoStack.push(action);
      return result;
    } finally { this.busy = false; }
  }
}
