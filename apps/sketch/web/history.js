/* Project: Privatium™ | File: apps/sketch/web/history.js
 * Authors: Gabriel Mongefranco (@gabrielmongefranco)
 * Created: 2026-09-06 | Modified: 2026-09-06
 * Summary: Session undo uses compensating events; original stroke order survives restoration.
 */
/** Retain up to 50 successful local actions. The supplied writer durably appends events. */
export class SketchHistory {
  constructor(write, mint) {
    this.write = write;
    this.mint = mint;
    this.strokes = new Map();
    this.order = new Map();
    this.undoStack = [];
    this.busy = false;
  }
  /** Apply an event from replay, subscription or a successful local write. */
  apply(ev) {
    if (ev.op === 'del') this.strokes.delete(ev.id);
    else {
      const layer = ev.d.layer || ev.id;
      if (!this.order.has(layer)) this.order.set(layer, this.order.size);
      this.strokes.set(ev.id, ev.d);
    }
  }
  /** Read visible strokes in their original painting order, including restored strokes. */
  entries() {
    return [...this.strokes].sort(([a, x], [b, y]) =>
      this.order.get(x.layer || a) - this.order.get(y.layer || b));
  }
  /** Append an action and remember its inverse only after success; reject concurrent writes. */
  async change(events) {
    if (this.busy) throw new Error('Wait for the current action to finish.');
    if (!events.length) return;
    const inverse = events.map(ev => this.strokes.has(ev.id)
      ? {op:'put', tbl:'stroke', id:ev.id, d:this.strokes.get(ev.id)}
      : {op:'del', tbl:'stroke', id:ev.id});
    this.busy = true;
    try {
      const result = await this.write(events);
      events.forEach(ev => this.apply(ev));
      this.undoStack.push({events, inverse});
      if (this.undoStack.length > 50) this.undoStack.shift();
      return result;
    } finally { this.busy = false; }
  }
  /** Undo one local action without overwriting a later change to its strokes. */
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
      inverse.forEach(ev => this.apply(ev));
      this.undoStack.pop();
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
