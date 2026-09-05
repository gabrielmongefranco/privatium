// Project:  Privatium™  |  File: crates/privatium-core/tests/js/pv.test.mjs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-05  |  Modified: 2026-09-05
// Summary:  pv.js against spec/data-api.md §5 and §6 and spec/protocol.md §10.6, under
//           `node --test`: the outbox queues while the node is unreachable and replays in
//           order when it is back; an empty replay leaves the helper able to replay
//           later; the node's own trouble keeps an entry and a refusal drops it; an
//           append during a replay is not lost; no storage is still a queue; an entry
//           already in the log is not sent again; an entry queued for another app is
//           refused; and the file stays under the size the spec promises.

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { page, storage, upNode, downNode, source } from './harness.mjs';

/** The appends that reached the node; an attempt the network dropped is not one. */
const posts = requests => requests.filter(r => r.method === 'POST' && r.path.endsWith('/api/events') && !r.failed);

/** The outbox as storage holds it: one key per entry beneath the mount, in id order. */
const stored = (store, mount = '/a/sketch/') =>
  [...store.map.keys()].filter(k => k.startsWith('pv:outbox:' + mount + ':')).sort().map(k => JSON.parse(store.map.get(k)));

/** A node whose identity is `id`, otherwise `upNode`. */
const nodeCalled = (id, app = 'sketch') => {
  const up = upNode(app);
  return (m, path, body) => (path.endsWith('/api/node') ? { json: { id, dev: id, name: 'Other', app, solo: false, peers: 0, restore_tier: 3 } } : up(m, path, body));
};

test('spec/data-api.md §5: pv.js is under the size the spec promises, and converts no DECIMAL', () => {
  const text = source();
  assert.ok(Buffer.byteLength(text, 'utf8') < 12 * 1024, `${Buffer.byteLength(text, 'utf8')} bytes`);
  for (const conversion of ['parseFloat(', 'Number(', '+row', 'toFixed(']) assert.ok(!text.includes(conversion), conversion);
});

test('spec/data-api.md §6: an empty replay at load leaves the outbox able to replay later', async () => {
  const p = await page({ respond: upNode() });
  await p.settle();
  assert.equal(posts(p.requests).length, 0, 'nothing waited');
  p.respond(downNode());
  const out = await p.pv.put('stroke', '01K4B0000000000000000000A1', { points: [[0, 0]] });
  assert.deepEqual(out, { queued: true, appended: 0, ids: ['01K4B0000000000000000000A1'] });
  assert.equal(p.pv.online, false);
  assert.equal(stored(p.store).length, 1);
  p.respond(upNode());
  p.fire('online');
  await p.pv.flush();
  assert.equal(posts(p.requests).length, 1, 'the queued write was sent');
  assert.equal(posts(p.requests)[0].body.events[0].id, '01K4B0000000000000000000A1');
  assert.equal(stored(p.store).length, 0, 'the queue is empty again');
});

test('spec/data-api.md §6: entries replay in order and carry the mark and the app they were queued under', async () => {
  const p = await page({ respond: upNode('sketch', { value: 40 }) });
  await p.settle();
  await p.pv.query('v_x').catch(() => {});                    // a 404 here; the mark comes from an append
  await p.pv.put('stroke', '01K4B0000000000000000000A1', { n: 1 });
  assert.equal(p.pv.lam, 41);
  p.respond(downNode());
  await p.pv.put('stroke', '01K4B0000000000000000000A2', { n: 2 });
  await p.pv.del('stroke', '01K4B0000000000000000000A1');
  const entries = stored(p.store);
  assert.equal(entries.length, 2);
  assert.equal(entries[0].lam, 41);
  assert.equal(entries[0].app, 'sketch');
  assert.equal(entries[0].node, 'k7m2q9xf', 'the node it was queued against');
  assert.match(entries[0].id, /^[0-9A-HJKMNP-TV-Z]{26}$/);
  p.respond(upNode('sketch', { value: 41 }));
  await p.pv.flush();
  const sent = posts(p.requests).slice(1);
  assert.equal(sent.length, 2);
  assert.equal(sent[0].body.events[0].id, '01K4B0000000000000000000A2');
  assert.equal(sent[1].body.events[0].op, 'del');
});

test('spec/data-api.md §6: a 5xx or 429 keeps the entry; a 4xx drops it and reports it', async () => {
  const p = await page({ respond: downNode(), online: false });
  const rejected = [];
  p.pv.on('rejected', e => rejected.push(e));
  await p.pv.put('stroke', '01K4B0000000000000000000A1', { n: 1 });
  await p.pv.put('stroke', '01K4B0000000000000000000A2', { n: 2 });
  const up = upNode();
  p.respond((m, path, body) => (m === 'POST' && path.endsWith('/api/events') ? { status: 503, json: { error: '503' } } : up(m, path, body)));
  p.fire('online');
  await p.pv.flush();
  assert.equal(stored(p.store).length, 2, 'kept for next time');
  assert.equal(rejected.length, 0);
  p.respond((m, path, body) => (m === 'POST' && path.endsWith('/api/events') ? { status: 429, json: { error: '429' } } : up(m, path, body)));
  await p.pv.flush();
  assert.equal(stored(p.store).length, 2, 'rate limited: kept');
  p.respond((m, path, body) => (m === 'POST' && path.endsWith('/api/events') ? { status: 409, json: { error: '409 Conflict: reused id', index: 0 } } : up(m, path, body)));
  await p.pv.flush();
  assert.equal(stored(p.store).length, 0, 'refused: dropped');
  assert.equal(rejected.length, 2);
  assert.equal(rejected[0].error.status, 409);
  assert.equal(rejected[0].events[0].id, '01K4B0000000000000000000A1');
});

test('spec/data-api.md §6: an append during a replay is queued behind it, not lost', async () => {
  const p = await page({ respond: downNode(), online: false });
  await p.pv.put('stroke', '01K4B0000000000000000000A1', { n: 1 });
  let release;
  const gate = new Promise(resolve => { release = resolve; });
  const up = upNode();
  p.respond(async (m, path, body) => { if (m === 'POST') await gate; return up(m, path, body); });
  p.fire('online');
  const replay = p.pv.flush();
  await p.settle();
  const second = await p.pv.put('stroke', '01K4B0000000000000000000A2', { n: 2 });
  assert.equal(second.queued, true, 'queued behind the replay in progress');
  release();
  await replay;
  await p.pv.flush();
  assert.deepEqual(posts(p.requests).map(r => r.body.events[0].id), ['01K4B0000000000000000000A1', '01K4B0000000000000000000A2']);
  assert.equal(stored(p.store).length, 0);
});

test('spec/data-api.md §6: with no storage the queue lives for the page', async () => {
  const p = await page({ respond: downNode(), online: false, store: storage(true) });
  const out = await p.pv.put('stroke', '01K4B0000000000000000000A1', { n: 1 });
  assert.equal(out.queued, true);
  p.respond(upNode());
  p.fire('online');
  await p.pv.flush();
  assert.equal(posts(p.requests).length, 1);
});

test('spec/protocol.md §10.6: an entry already in the log past its mark is not sent again', async () => {
  const p = await page({ respond: upNode('sketch', { value: 7 }) });
  await p.settle();
  await p.pv.put('stroke', '01K4B0000000000000000000A0', { n: 0 });   // the mark is 8 now
  p.respond(downNode());
  await p.pv.put('stroke', '01K4B0000000000000000000A1', { points: [[1, 2]], color: '#00274C' });
  await p.pv.del('stroke', '01K4B0000000000000000000A0');
  const up = upNode('sketch', { value: 9 });
  p.respond((m, path, body) => {
    if (m === 'GET' && path.includes('/api/events?')) {
      const q = new URL('http://x' + path).searchParams;
      assert.equal(q.get('after'), '8');
      const id = q.get('id');
      // The node has both: the put landed before the response was lost, with its keys in another order; the del too.
      if (id === '01K4B0000000000000000000A1') return { text: JSON.stringify({ seq: 9, lam: 9, op: 'put', tbl: 'stroke', id, d: { color: '#00274C', points: [[1, 2]] } }) + '\n' };
      if (id === '01K4B0000000000000000000A0') return { text: JSON.stringify({ seq: 10, lam: 10, op: 'del', tbl: 'stroke', id }) + '\n' };
    }
    return up(m, path, body);
  });
  p.fire('online');
  await p.pv.flush();
  assert.equal(posts(p.requests).length, 1, 'only the first, online, put was ever posted');
  assert.equal(stored(p.store).length, 0);
});

test('spec/protocol.md §10.6: a row that moved since the entry was queued is a conflict — refused and reported, never written over', async () => {
  const p = await page({ respond: downNode(), online: false });
  const rejected = [];
  p.pv.on('rejected', e => rejected.push(e));
  // Queued offline: an edit of A, and a brand-new row B.
  await p.pv.put('stroke', '01K4B0000000000000000000A1', { n: 2 });
  await p.pv.put('stroke', '01K4B0000000000000000000B1', { n: 9 });
  const up = upNode();
  p.respond((m, path, body) => {
    if (m === 'GET' && path.includes('/api/events?')) {
      const id = new URL('http://x' + path).searchParams.get('id');
      // Another device edited A after the mark; nobody has touched B.
      if (id === '01K4B0000000000000000000A1') return { text: JSON.stringify({ op: 'put', tbl: 'stroke', id, d: { n: 1 } }) + '\n' };
    }
    return up(m, path, body);
  });
  p.fire('online');
  await p.pv.flush();
  assert.equal(rejected.length, 1);
  assert.equal(rejected[0].error.status, 409);
  assert.deepEqual(rejected[0].error.conflict, { tbl: 'stroke', id: '01K4B0000000000000000000A1' });
  assert.match(rejected[0].error.message, /newer change to stroke\/01K4B0000000000000000000A1/);
  assert.deepEqual(posts(p.requests).map(r => r.body.events[0].id), ['01K4B0000000000000000000B1'], 'the fresh row went, the stale edit did not');
  assert.equal(stored(p.store).length, 0);
});

test('spec/data-api.md §6: in solo mode an entry queued before the app was known is refused; in host mode the mount names it', async () => {
  // Solo mount, first load with the node down and nothing cached: the app is unknown.
  const solo = await page({ pathname: '/', respond: downNode(), online: false });
  const rejected = [];
  solo.pv.on('rejected', e => rejected.push(e));
  await solo.pv.put('save', '01K4B0000000000000000000A1', { level: 7 });
  assert.equal(stored(solo.store, '/')[0].app, null);
  solo.respond(upNode('mygame'));
  solo.fire('online');
  await solo.pv.flush();
  assert.equal(posts(solo.requests).length, 0, 'never replayed into whichever app owns / now');
  assert.equal(rejected.length, 1);
  assert.match(rejected[0].error.message, /before the app at this mount was known/);

  // A host-mode mount carries the app in its path, node or no node.
  const host = await page({ pathname: '/a/sketch/', respond: downNode(), online: false });
  await host.pv.put('stroke', '01K4B0000000000000000000A1', { n: 1 });
  assert.equal(stored(host.store)[0].app, 'sketch');
  host.respond(upNode('sketch'));
  host.fire('online');
  await host.pv.flush();
  assert.equal(posts(host.requests).length, 1);
});

test('spec/data-api.md §6: an entry queued for another app at this mount is refused, never replayed; a one-list queue from an earlier helper is carried over', async () => {
  const store = storage();
  store.setItem('pv:app:/', JSON.stringify('other'));
  store.setItem('pv:outbox:/', JSON.stringify([{ id: '01K4B0000000000000000000B1', lam: 3, app: 'other', events: [{ op: 'put', tbl: 'save', id: '01K4B0000000000000000000A1', d: { level: 7 } }] }]));
  const p = await page({ pathname: '/', store, respond: upNode('mygame') });
  const rejected = [];
  p.pv.on('rejected', e => rejected.push(e));
  assert.equal(store.map.has('pv:outbox:/'), false, 'the one-list key is carried over and removed at load');
  await p.settle();
  await p.pv.flush();
  assert.equal(posts(p.requests).length, 0);
  assert.equal(rejected.length, 1);
  assert.match(rejected[0].error.message, /queued for app other/);
  assert.equal(JSON.parse(store.map.get('pv:app:/')), 'mygame', 'the app served now is remembered');
  assert.equal(stored(store, '/').length, 0);
});

test('spec/data-api.md §5: a put with no id is minted one before it is sent or queued', async () => {
  const p = await page({ respond: downNode(), online: false });
  const out = await p.pv.append([{ op: 'put', tbl: 'stroke', d: { n: 1 } }]);
  assert.equal(out.queued, true);
  assert.match(out.ids[0], /^[0-9A-HJKMNP-TV-Z]{26}$/);
  assert.equal(stored(p.store)[0].events[0].id, out.ids[0]);
});

test('spec/data-api.md §5: a row read with pv.get moves the mark, so an edit of that row is not a false conflict', async () => {
  const p = await page({ respond: upNode() });
  await p.settle();
  const up = upNode();
  p.respond((m, path, body) => {
    if (m === 'GET' && path.endsWith('/api/row/save/01K4B0000000000000000000A1')) return { text: JSON.stringify({ seq: 7, lam: 7, op: 'put', tbl: 'save', id: '01K4B0000000000000000000A1', d: { level: 1 } }) };
    return up(m, path, body);
  });
  const row = await p.pv.get('save', '01K4B0000000000000000000A1');
  assert.equal(row.d.level, 1);
  assert.equal(p.pv.lam, 7, 'the mark follows the row the page saw');
  const missing = await p.pv.get('save', '01K4B0000000000000000000A2');
  assert.equal(missing, null);
});

test('spec/data-api.md §6: an entry is bound to the node it was queued against; a replay asks who serves the origin now and refuses another node', async () => {
  // A page that learned node k7m2q9xf, queued while it was unreachable, and finds a
  // different data root answering on the same port afterwards.
  const p = await page({ respond: upNode() });
  await p.settle();
  p.respond(downNode());
  await p.pv.put('stroke', '01K4B0000000000000000000A1', { n: 1 });
  assert.equal(stored(p.store)[0].node, 'k7m2q9xf');
  const rejected = [];
  p.pv.on('rejected', e => rejected.push(e));
  p.respond(nodeCalled('newnode0'));
  p.fire('online');
  await p.pv.flush();
  assert.equal(posts(p.requests).length, 0, 'never replayed into another node');
  assert.equal(rejected.length, 1);
  assert.match(rejected[0].error.message, /queued for node k7m2q9xf; this origin now serves newnode0/);
  assert.equal(JSON.parse(p.store.map.get('pv:node:/a/sketch/')), 'newnode0', 'the node served now is remembered');
  assert.equal(stored(p.store).length, 0);

  // An entry that matches replays, and the POST names the node and the app it is for, so
  // the node can refuse a mismatch itself.
  p.respond(downNode());
  await p.pv.put('stroke', '01K4B0000000000000000000A2', { n: 2 });
  assert.equal(stored(p.store)[0].node, 'newnode0');
  p.respond(nodeCalled('newnode0'));
  p.fire('online');
  await p.pv.flush();
  const sent = posts(p.requests);
  assert.equal(sent.length, 1);
  assert.equal(sent[0].body.node, 'newnode0');
  assert.equal(sent[0].body.app, 'sketch');
});

test('spec/data-api.md §6: two pages over one storage keep both queues — one key per entry — and either replays both', async () => {
  const store = storage();
  const a = await page({ store, respond: downNode(), online: false });
  await a.pv.put('stroke', '01K4B0000000000000000000A1', { n: 1 });
  const b = await page({ store, respond: downNode(), online: false });
  await b.pv.put('stroke', '01K4B0000000000000000000B1', { n: 2 });
  assert.equal(stored(store).length, 2, `both pages' entries are stored: ${[...store.map.keys()]}`);
  // Page a closed while offline; page b comes back and replays both, oldest first.
  b.respond(upNode());
  b.fire('online');
  await b.pv.flush();
  assert.deepEqual(posts(b.requests).map(r => r.body.events[0].id), ['01K4B0000000000000000000A1', '01K4B0000000000000000000B1']);
  assert.equal(stored(store).length, 0, 'the queue is empty again');
});
