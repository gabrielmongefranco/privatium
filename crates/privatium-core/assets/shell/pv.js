/*
 * Project:  Privatium™  |  File: crates/privatium-core/assets/shell/pv.js
 * Authors:  Gabriel Mongefranco (@gabrielmongefranco)
 * Created:  2026-09-03  |  Modified: 2026-09-03
 * Summary:  The data API helper of spec/data-api.md §5, served at /static/pv.js. A plain
 *           ES module with no dependencies and no build step: query, sql, get, events,
 *           append, put, del, subscribe, ulid, node, url, online, on. Writes queue in an
 *           outbox keyed by ULID while the node is unreachable and replay exactly as they
 *           were — nothing records what landed and nothing acknowledges, because the ULID
 *           makes a replay converge (spec/protocol.md §10.6). DECIMAL and BIGINT columns
 *           arrive as strings and stay strings.
 */
const MOUNT = (() => {
  const m = location.pathname.match(/^\/a\/[a-z][a-z0-9-]{1,30}\//);
  return m ? m[0] : '/';
})();
const OUTBOX = 'pv:outbox:' + MOUNT;
const ALPHABET = '0123456789ABCDEFGHJKMNPQRSTVWXYZ';

export class PvOffline extends Error {
  constructor(message) { super(message || 'the node is unreachable'); this.name = 'PvOffline'; }
}

const state = { online: navigator.onLine !== false, lam: 0, node: null, es: null, delay: 1000 };
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
    throw err;
  }
  return res;
}

// ---- the outbox: entries keyed by a ULID, replayed as they are -------------------------
function load() { try { return JSON.parse(localStorage.getItem(OUTBOX)) || []; } catch { return []; } }
function save(queue) {
  try { if (queue.length) localStorage.setItem(OUTBOX, JSON.stringify(queue)); else localStorage.removeItem(OUTBOX); }
  catch { /* no storage: the queue lives for the page */ }
}
let flushing = null;
function flush() {
  if (flushing) return flushing;
  flushing = (async () => {
    const queue = load();
    while (queue.length) {
      const entry = queue[0];
      try { noteLam((await (await call('POST', 'events', { events: entry.events })).json()).lam); }
      catch (e) {
        if (e instanceof PvOffline) break;                       // still unreachable: keep it
        emit('rejected', { id: entry.id, events: entry.events, error: e }); // refused: nothing to retry
      }
      queue.shift(); save(queue);
    }
    flushing = null;
  })();
  return flushing;
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
  const queue = load();
  if (state.online && !queue.length) {
    try { const out = await (await call('POST', 'events', { events: list })).json(); noteLam(out.lam); return out; }
    catch (e) { if (!(e instanceof PvOffline)) throw e; }
  }
  queue.push({ id: ulid(), events: list }); save(queue);
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
  if (!state.node) state.node = await (await call('GET', 'node')).json();
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
if (state.online) flush();

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
