/*
 * Project:  Privatium™  |  File: crates/privatium-core/assets/shell/pv.js
 * Authors:  Gabriel Mongefranco (@gabrielmongefranco)
 * Created:  2026-09-03  |  Modified: 2026-09-05
 * Summary:  The data API helper of spec/data-api.md §5, served at /static/pv.js. A plain
 *           ES module with no dependencies and no build step: query, sql, get, events,
 *           append, put, del, subscribe, ulid, node, url, online, on. Writes queue in an
 *           outbox while the node is unreachable and replay exactly as they were: an entry
 *           carries the high-water mark and the app it was queued under, and before a
 *           replay the row's events past that mark are read — already there, it is
 *           dropped; moved since, it is refused and reported; nothing, it is sent
 *           (spec/protocol.md §10.6). Nothing else is remembered. DECIMAL stays a string.
 */
const MOUNT = (() => {
  const m = location.pathname.match(/^\/a\/[a-z][a-z0-9-]{1,30}\//);
  return m ? m[0] : '/';
})();
const MOUNT_APP = MOUNT === '/' ? null : MOUNT.slice(3, -1);   // host mode names the app in the path
const OUTBOX = 'pv:outbox:' + MOUNT, APP = 'pv:app:' + MOUNT;
const ALPHABET = '0123456789ABCDEFGHJKMNPQRSTVWXYZ';

export class PvOffline extends Error {
  constructor(message) { super(message || 'the node is unreachable'); this.name = 'PvOffline'; }
}

function read(key) { try { return JSON.parse(localStorage.getItem(key)); } catch { return null; } }
function write(key, value) {
  try { if (value == null) localStorage.removeItem(key); else localStorage.setItem(key, JSON.stringify(value)); }
  catch { /* no storage: the queue lives for the page */ }
}

const state = { online: navigator.onLine !== false, lam: 0, node: null, app: MOUNT_APP || read(APP), es: null, delay: 1000 };
const handlers = {};
const subscribers = new Set();
// The outbox: one queue in memory — the truth for this page — mirrored to storage when it can be.
let queue = read(OUTBOX);
if (!Array.isArray(queue)) queue = [];

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
    throw err;
  }
  return res;
}

// ---- the outbox: entries keyed by a ULID, replayed as they are -------------------------
function persist() { write(OUTBOX, queue.length ? queue : null); }
function refuse(message, conflict) { const err = new Error(message); err.status = 409; if (conflict) err.conflict = conflict; throw err; }
function same(a, b) {
  if (a === b) return true;
  if (typeof a !== 'object' || typeof b !== 'object' || a === null || b === null) return false;
  if (Array.isArray(a) !== Array.isArray(b)) return false;
  const ka = Object.keys(a);
  return ka.length === Object.keys(b).length && ka.every(k => Object.hasOwn(b, k) && same(a[k], b[k]));
}
// Where the entry stands, read from the log past the mark it was queued at — never remembered
// (spec/protocol.md §10.6): every event already there is 'landed'; another event on one of its
// rows is a conflict, named; nothing on any row is 'fresh'.
async function stand(entry) {
  let landed = 0;
  for (const ev of entry.events) {
    const text = await (await call('GET', 'events' + qs({ tbl: ev.tbl, id: ev.id, after: entry.lam }))).text();
    const lines = text.split('\n').filter(l => l.trim()).map(l => JSON.parse(l));
    if (lines.some(line => line.op === ev.op && (ev.op === 'del' || same(line.d, ev.d)))) landed++;
    else if (lines.length) return { conflict: { tbl: ev.tbl, id: ev.id } };
  }
  return landed === entry.events.length ? 'landed' : 'fresh';
}
let flushing = null;
function flush() {
  if (flushing) return flushing;
  const run = (async () => {
    while (queue.length) {
      const entry = queue[0];
      try {
        if (!state.app) await node();                       // which app this origin serves now
        if (!entry.app) refuse('queued before the app at this mount was known; not replayed');
        if (entry.app !== state.app) refuse('queued for app ' + entry.app + '; this origin now serves ' + state.app);
        const where = await stand(entry);
        if (where.conflict) refuse('a newer change to ' + where.conflict.tbl + '/' + where.conflict.id + ' landed after this was queued', where.conflict);
        if (where !== 'landed') noteLam((await (await call('POST', 'events', { events: entry.events })).json()).lam);
      } catch (e) {
        // Unreachable, or the node's trouble rather than the entry's: keep it for next time.
        if (e instanceof PvOffline || e.status >= 500 || e.status === 429 || e.status === 408) break;
        emit('rejected', { id: entry.id, events: entry.events, error: e });   // refused: nothing to retry
      }
      queue.shift(); persist();
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
    try { const out = await (await call('POST', 'events', { events: list })).json(); noteLam(out.lam); return out; }
    catch (e) { if (!(e instanceof PvOffline)) throw e; }
  }
  queue.push({ id: ulid(), lam: state.lam, app: state.app, events: list }); persist();
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
  try { return await (await call('GET', 'row/' + encodeURIComponent(tbl) + '/' + encodeURIComponent(id))).json(); }
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
      if (line.trim()) { const ev = JSON.parse(line); noteLam(ev.lam); yield ev; }
    }
    if (done) break;
  }
  if (buffer.trim()) { const ev = JSON.parse(buffer); noteLam(ev.lam); yield ev; }
}
async function node() {
  if (!state.node) {
    state.node = await (await call('GET', 'node')).json();
    if (state.node.app) { state.app = state.node.app; write(APP, state.app); }
  }
  return state.node;
}

// ---- live updates: EventSource, reconnected by hand so after= carries the last lam ------
function connect() {
  if (state.es || !subscribers.size) return;
  const es = new EventSource(url('api/stream' + (state.lam ? '?after=' + state.lam : '')));
  state.es = es;
  es.addEventListener('append', e => { const ev = JSON.parse(e.data); noteLam(ev.lam); for (const fn of subscribers) fn(ev); });
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

function ulid() {
  let t = Date.now(), out = '';
  for (let i = 0; i < 10; i++) { out = ALPHABET[t % 32] + out; t = Math.floor(t / 32); }
  const r = new Uint8Array(16); crypto.getRandomValues(r);   // available on plain HTTP; crypto.subtle is not
  for (let i = 0; i < 16; i++) out += ALPHABET[r[i] & 31];
  return out;
}

addEventListener('online', () => setOnline(true));
addEventListener('offline', () => setOnline(false));
if (state.online) node().catch(() => {}).then(flush);   // learn the app, then replay what waited

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
