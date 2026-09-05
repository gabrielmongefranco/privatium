/*
 * Project:  Privatium™  |  File: crates/privatium-core/assets/shell/pv.js
 * Authors:  Gabriel Mongefranco (@gabrielmongefranco)
 * Created:  2026-09-03  |  Modified: 2026-09-05
 * Summary:  The data API helper of spec/data-api.md §5, served at /static/pv.js. A plain
 *           ES module with no dependencies and no build step: query, sql, get, events,
 *           append, put, del, subscribe, ulid, node, url, online, on. Writes queue in an
 *           outbox while the node is unreachable and replay exactly as they were: an entry
 *           carries the high-water mark, the rank of each row the page had seen, the app
 *           and the node it was queued under; the node judges the replay under its lock —
 *           landed, dropped; moved since, refused and reported; nothing, appended
 *           (spec/protocol.md §10.6). DECIMAL stays a string.
 */
const MOUNT = (() => {
  const m = location.pathname.match(/^\/a\/[a-z][a-z0-9-]{1,30}\//);
  return m ? m[0] : '/';
})();
const MOUNT_APP = MOUNT === '/' ? null : MOUNT.slice(3, -1);   // host mode names the app in the path
const OUTBOX = 'pv:outbox:' + MOUNT, APP = 'pv:app:' + MOUNT, NODE = 'pv:node:' + MOUNT;
const ALPHABET = '0123456789ABCDEFGHJKMNPQRSTVWXYZ';

export class PvOffline extends Error {
  constructor(message) { super(message || 'the node is unreachable'); this.name = 'PvOffline'; }
}

function read(key) { try { return JSON.parse(localStorage.getItem(key)); } catch { return null; } }
function write(key, value) {
  try { if (value == null) localStorage.removeItem(key); else localStorage.setItem(key, JSON.stringify(value)); }
  catch { /* no storage: the queue lives for the page */ }
}

const state = { online: navigator.onLine !== false, lam: 0, node: null, app: MOUNT_APP || read(APP), nodeId: read(NODE), es: null, delay: 1000 };
const handlers = {};
const subscribers = new Set();

function emit(event, data) {
  for (const fn of handlers[event] || []) { try { fn(data); } catch (e) { console.error(e); } }
}
function setOnline(online) {
  if (state.online === online) return;
  state.online = online;
  emit(online ? 'online' : 'offline');
  if (online) flush();
}
function noteLam(lam) { if (typeof lam === 'number' && lam > state.lam) state.lam = lam; }
function url(path) { return MOUNT + String(path == null ? '' : path).replace(/^\/+/, ''); }
function qs(params) {
  const q = new URLSearchParams();
  for (const [k, v] of Object.entries(params || {})) if (v != null) q.set(k, v);
  const s = q.toString();
  return s ? '?' + s : '';
}

async function call(method, path, body) {
  const init = { method, credentials: 'same-origin', headers: {} };
  if (body !== undefined) { init.headers['content-type'] = 'application/json'; init.body = JSON.stringify(body); }
  let res;
  try { res = await fetch(url('api/' + path), init); }
  catch (e) { setOnline(false); throw new PvOffline(e.message); }
  setOnline(true);
  if (!res.ok) {
    let detail = null;
    try { detail = await res.json(); } catch { /* not JSON */ }
    const err = new Error((detail && detail.error) || res.status + ' ' + res.statusText);
    err.status = res.status; err.detail = detail;
    if (detail && detail.conflict) err.conflict = detail.conflict;   // the node refused a replay: the row moved (§6)
    throw err;
  }
  return res;
}

// ---- what the page has seen: the rank of every row it read or wrote (§5) -----------------
const seen = new Map();
function saw(ev) { if (ev && ev.tbl && ev.id && typeof ev.lam === 'number') seen.set(ev.tbl + '/' + ev.id, { lam: ev.lam, ts: ev.ts, dev: ev.dev }); }
// A batch the node appended has contiguous lams from the response's, one ts, this node; one it
// found already landed leaves the page knowing no rank for those rows until it reads them again.
function wrote(events, out) {
  if (!out || typeof out.lam !== 'number') return;
  const first = out.lam - events.length + 1;
  events.forEach((ev, i) => (out.appended ? saw({ tbl: ev.tbl, id: ev.id, lam: first + i, ts: out.ts, dev: out.dev }) : seen.delete(ev.tbl + '/' + ev.id)));
}

// ---- the outbox: one queue in memory, the truth for this page, one storage key per entry --
let queue = [];
function persist(entry) { write(OUTBOX + ':' + entry.id, entry); }
function forget(entry) { write(OUTBOX + ':' + entry.id, null); }
function stored() {
  const entries = [];
  try {
    for (let i = 0; i < localStorage.length; i++) {
      const key = localStorage.key(i);
      if (key && key.startsWith(OUTBOX + ':')) { const entry = read(key); if (entry && entry.id) entries.push(entry); }
    }
    const legacy = read(OUTBOX);                              // an earlier helper's one-list key
    if (Array.isArray(legacy)) { for (const entry of legacy) if (entry && entry.id) { entries.push(entry); persist(entry); } write(OUTBOX, null); }
  } catch { /* no storage */ }
  return entries;
}
// Another page's entries join this one's, oldest first: the ids are ULIDs.
function adopt() {
  const known = new Set(queue.map(entry => entry.id));
  for (const entry of stored()) if (!known.has(entry.id)) queue.push(entry);
  queue.sort((a, b) => (a.id < b.id ? -1 : a.id > b.id ? 1 : 0));
}
adopt();
// The POST body: the events, and the node and the app they are for when known (§2).
function body(events, node, app) { const b = { events }; if (node) b.node = node; if (app) b.app = app; return b; }
function refuse(message) { const err = new Error(message); err.status = 409; throw err; }
let flushing = null;
function flush() {
  if (flushing) return flushing;
  const run = (async () => {
    adopt();
    let learned = false;
    while (queue.length) {
      const entry = queue[0];
      try {
        if (!learned) { await learn(); learned = true; }   // who serves this origin now, not at load
        if (!entry.app) refuse('queued before the app at this mount was known; not replayed');
        if (entry.app !== state.app) refuse('queued for app ' + entry.app + '; this origin now serves ' + state.app);
        if (entry.node && entry.node !== state.nodeId) refuse('queued for node ' + entry.node + '; this origin now serves ' + state.nodeId);
        // The node judges it against the log, under its lock (spec/protocol.md §10.6): the mark
        // the entry was queued at, and each row's rank as the page saw it, go with the events.
        const out = await (await call('POST', 'events', { ...body(entry.events, entry.node, entry.app), since: entry.lam })).json();
        noteLam(out.lam); wrote(entry.events, out);
      } catch (e) {
        // Unreachable, or the node's trouble rather than the entry's: keep it for next time.
        if (e instanceof PvOffline || e.status >= 500 || e.status === 429 || e.status === 408) break;
        emit('rejected', { id: entry.id, events: entry.events, error: e });   // refused: nothing to retry
      }
      forget(entry); queue.shift();
    }
  })();
  flushing = run;
  run.finally(() => { if (flushing === run) flushing = null; });
  return run;
}

async function append(events) {
  const list = events.map(ev => {
    const out = { op: ev.op, tbl: ev.tbl };
    if (ev.id != null) out.id = ev.id;
    else if (ev.op === 'put') out.id = ulid();   // minted here, so a replay carries the same id
    if (ev.d !== undefined) out.d = ev.d;
    return out;
  });
  const ids = list.map(ev => ev.id);
  if (state.online && !queue.length) {
    try { const out = await (await call('POST', 'events', body(list, state.nodeId, state.app))).json(); noteLam(out.lam); wrote(list, out); return out; }
    catch (e) { if (!(e instanceof PvOffline)) throw e; }
  }
  // Queued as it will be sent, each row with the rank the page saw for it, if any (§5).
  const carried = list.map(ev => { const base = seen.get(ev.tbl + '/' + ev.id); return base ? { ...ev, base } : ev; });
  const entry = { id: ulid(), lam: state.lam, app: state.app, node: state.nodeId, events: carried };
  queue.push(entry); persist(entry);
  if (state.online) flush();
  return { queued: true, appended: 0, ids };
}

async function query(view, params) {
  const out = await (await call('GET', 'q/' + encodeURIComponent(view) + qs(params))).json();
  noteLam(out.lam); return out.rows;
}
async function sql(text, params) {
  const out = await (await call('POST', 'sql', { sql: text, params: params || [] })).json();
  noteLam(out.lam); return out.rows;
}
async function get(tbl, id) {
  try { const ev = await (await call('GET', 'row/' + encodeURIComponent(tbl) + '/' + encodeURIComponent(id))).json(); noteLam(ev.lam); saw(ev); return ev; }
  catch (e) { if (e.status === 404) return null; throw e; }
}
async function* events(filter) {
  const res = await call('GET', 'events' + qs(filter));
  const reader = res.body.getReader(), decoder = new TextDecoder();
  let buffer = '';
  for (;;) {
    const { value, done } = await reader.read();
    buffer += decoder.decode(value || new Uint8Array(), { stream: !done });
    let at;
    while ((at = buffer.indexOf('\n')) >= 0) {
      const line = buffer.slice(0, at); buffer = buffer.slice(at + 1);
      if (line.trim()) { const ev = JSON.parse(line); noteLam(ev.lam); saw(ev); yield ev; }
    }
    if (done) break;
  }
  if (buffer.trim()) { const ev = JSON.parse(buffer); noteLam(ev.lam); saw(ev); yield ev; }
}
// /api/node, asked now: the app and the node this origin serves, remembered for an unreachable load.
async function learn() {
  state.node = await (await call('GET', 'node')).json();
  if (state.node.app) { state.app = state.node.app; write(APP, state.app); }
  if (state.node.id) { state.nodeId = state.node.id; write(NODE, state.nodeId); }
  return state.node;
}
async function node() { return state.node || learn(); }

// ---- live updates: EventSource, reconnected by hand so after= carries the last lam ------
function connect() {
  if (state.es || !subscribers.size) return;
  const es = new EventSource(url('api/stream' + (state.lam ? '?after=' + state.lam : '')));
  state.es = es;
  es.addEventListener('append', e => { const ev = JSON.parse(e.data); noteLam(ev.lam); saw(ev); for (const fn of subscribers) fn(ev); });
  es.addEventListener('resync', e => { const d = JSON.parse(e.data); noteLam(d.lam); emit('resync', d); });
  es.addEventListener('ping', e => noteLam(JSON.parse(e.data).lam));
  es.onopen = () => { state.delay = 1000; setOnline(true); };
  es.onerror = () => {
    es.close(); state.es = null;
    setTimeout(connect, state.delay); state.delay = Math.min(state.delay * 2, 30000);
  };
}
function subscribe(fn) {
  subscribers.add(fn); connect();
  return () => { subscribers.delete(fn); if (!subscribers.size && state.es) { state.es.close(); state.es = null; } };
}

// Monotonic within the page: a second ULID in the same millisecond increments the last one's
// random tail, so ids minted in order sort in order — which is what the outbox replays by.
let lastMs = 0, lastTail = null;
function ulid() {
  const now = Date.now();
  let tail;
  if (now === lastMs && lastTail) {
    tail = lastTail;
    for (let i = 15; i >= 0; i--) { if (tail[i] === 31) tail[i] = 0; else { tail[i]++; break; } }
  } else {
    const bytes = new Uint8Array(16); crypto.getRandomValues(bytes);   // available on plain HTTP; crypto.subtle is not
    tail = Array.from(bytes, b => b & 31);
  }
  lastMs = now; lastTail = tail;
  let t = now, out = '';
  for (let i = 0; i < 10; i++) { out = ALPHABET[t % 32] + out; t = Math.floor(t / 32); }
  for (let i = 0; i < 16; i++) out += ALPHABET[tail[i]];
  return out;
}

addEventListener('online', () => setOnline(true));
addEventListener('offline', () => setOnline(false));
if (state.online) node().catch(() => {}).then(flush);   // learn the app and the node, then replay what waited

export const pv = {
  query, sql, get, events, append, subscribe, ulid, node, url, flush,
  put: (tbl, id, d) => append([{ op: 'put', tbl, id, d }]),
  del: (tbl, id) => append([{ op: 'del', tbl, id }]),
  on(event, fn) { (handlers[event] ||= []).push(fn); return () => { handlers[event] = handlers[event].filter(f => f !== fn); }; },
  get online() { return state.online; },
  get lam() { return state.lam; },
  get mount() { return MOUNT; },
};
export default pv;
