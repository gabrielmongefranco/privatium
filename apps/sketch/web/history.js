/* Project: Privatium™ | File: apps/sketch/web/history.js
 * Authors: Gabriel Mongefranco (@gabrielmongefranco)
 * Created: 2026-09-06 | Modified: 2026-09-07
 * Summary: Undo over the shared canvas: every change the tab sees — its own, another
 *          window's, another device's, and the log replayed at load — joins one bounded
 *          history in the order it arrived, and undo reverses the latest with compensating
 *          events. Original stroke order survives restoration.
 *          See main README.md for full license information.
 */
/** Retain up to 50 changes to the canvas. The supplied writer durably appends events. */
export class SketchHistory {
  constructor(write, mint) {
    this.write = write;
    this.mint = mint;
    this.strokes = new Map();
    this.order = new Map();
    this.undoStack = [];
    this.busy = false;
  }
  /**
   * Apply an event from replay, subscription or a successful local write, and remember
   * how to reverse it. An event that changes nothing — this tab's own write echoed back
   * by the stream, a replayed line already applied — is not remembered. Events that
   * share one `ts` and one `dev` were written as one batch and are undone as one.
   */
  apply(ev, remember = true) {
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
    this.undoStack.push(entry);
    if (this.undoStack.length > 50) this.undoStack.shift();
  }
  /** Append an action from this tab and remember its inverse only after success; reject concurrent writes. */
  async change(events) {
    if (this.busy) throw new Error('Wait for the current action to finish.');
    if (!events.length) return;
    const inverse = events.map(ev => this.strokes.has(ev.id)
      ? {op:'put', tbl:'stroke', id:ev.id, d:this.strokes.get(ev.id)}
      : {op:'del', tbl:'stroke', id:ev.id});
    this.busy = true;
    try {
      const result = await this.write(events);
      events.forEach(ev => this.apply(ev, false));
      this.remember({events, inverse, batch:null});
      return result;
    } finally { this.busy = false; }
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
      const restored = new Map();
      const inverse = action.inverse.map(ev => {
        if (ev.op !== 'put' || this.strokes.has(ev.id)) return ev;
        const replacement = {...ev, id:this.mint(), d:{...ev.d, layer:ev.d.layer || ev.id}};
        restored.set(ev.id, replacement);
        return replacement;
      });
      const result = await this.write(inverse);
      this.undoStack.pop();
      inverse.forEach(ev => this.apply(ev, false));
      // Older undo entries must follow a restored stroke's new, never-reused ID.
      for (const entry of this.undoStack) {
        for (const list of [entry.events, entry.inverse]) {
          for (const ev of list) {
            const replacement = restored.get(ev.id);
            if (!replacement) continue;
            ev.id = replacement.id;
            if (ev.op === 'put') ev.d = {...ev.d, layer:replacement.d.layer};
          }
        }
      }
      return result;
    } finally { this.busy = false; }
  }
}
