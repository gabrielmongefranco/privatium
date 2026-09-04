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

test('spec/data-api.md §5: pv.js is under the size the spec promises, and converts no DECIMAL', () => {
  const text = source();
  assert.ok(Buffer.byteLength(text, 'utf8') < 10 * 1024, `${Buffer.byteLength(text, 'utf8')} bytes`);
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
  assert.equal(JSON.parse(p.store.map.get('pv:outbox:/a/sketch/')).length, 1);
  p.respond(upNode());
  p.fire('online');
  await p.pv.flush();
  assert.equal(posts(p.requests).length, 1, 'the queued write was sent');
  assert.equal(posts(p.requests)[0].body.events[0].id, '01K4B0000000000000000000A1');
  assert.equal(p.store.map.has('pv:outbox:/a/sketch/'), false, 'the queue is empty again');
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
  const stored = JSON.parse(p.store.map.get('pv:outbox:/a/sketch/'));
  assert.equal(stored.length, 2);
  assert.equal(stored[0].lam, 41);
  assert.equal(stored[0].app, 'sketch');
  assert.match(stored[0].id, /^[0-9A-HJKMNP-TV-Z]{26}$/);
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
  assert.equal(JSON.parse(p.store.map.get('pv:outbox:/a/sketch/')).length, 2, 'kept for next time');
  assert.equal(rejected.length, 0);
  p.respond((m, path, body) => (m === 'POST' && path.endsWith('/api/events') ? { status: 429, json: { error: '429' } } : up(m, path, body)));
  await p.pv.flush();
  assert.equal(JSON.parse(p.store.map.get('pv:outbox:/a/sketch/')).length, 2, 'rate limited: kept');
  p.respond((m, path, body) => (m === 'POST' && path.endsWith('/api/events') ? { status: 409, json: { error: '409 Conflict: reused id', index: 0 } } : up(m, path, body)));
  await p.pv.flush();
  assert.equal(p.store.map.has('pv:outbox:/a/sketch/'), false, 'refused: dropped');
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
  assert.equal(p.store.map.has('pv:outbox:/a/sketch/'), false);
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
  assert.equal(p.store.map.has('pv:outbox:/a/sketch/'), false);
});

test('spec/protocol.md §10.6: a different value for the row is not "landed" and is sent', async () => {
  const p = await page({ respond: downNode(), online: false });
  await p.pv.put('stroke', '01K4B0000000000000000000A1', { n: 2 });
  const up = upNode();
  p.respond((m, path, body) => (m === 'GET' && path.includes('/api/events?')
    ? { text: JSON.stringify({ op: 'put', tbl: 'stroke', id: '01K4B0000000000000000000A1', d: { n: 1 } }) + '\n' }
    : up(m, path, body)));
  p.fire('online');
  await p.pv.flush();
  assert.equal(posts(p.requests).length, 1);
});

test('spec/data-api.md §6: an entry queued for another app at this mount is refused, never replayed', async () => {
  const store = storage();
  store.setItem('pv:app:/', JSON.stringify('other'));
  store.setItem('pv:outbox:/', JSON.stringify([{ id: '01K4B0000000000000000000B1', lam: 3, app: 'other', events: [{ op: 'put', tbl: 'save', id: '01K4B0000000000000000000A1', d: { level: 7 } }] }]));
  const p = await page({ pathname: '/', store, respond: upNode('mygame') });
  const rejected = [];
  p.pv.on('rejected', e => rejected.push(e));
  await p.settle();
  await p.pv.flush();
  assert.equal(posts(p.requests).length, 0);
  assert.equal(rejected.length, 1);
  assert.match(rejected[0].error.message, /queued for app other/);
  assert.equal(JSON.parse(store.map.get('pv:app:/')), 'mygame', 'the app served now is remembered');
});

test('spec/data-api.md §5: a put with no id is minted one before it is sent or queued', async () => {
  const p = await page({ respond: downNode(), online: false });
  const out = await p.pv.append([{ op: 'put', tbl: 'stroke', d: { n: 1 } }]);
  assert.equal(out.queued, true);
  assert.match(out.ids[0], /^[0-9A-HJKMNP-TV-Z]{26}$/);
  assert.equal(JSON.parse(p.store.map.get('pv:outbox:/a/sketch/'))[0].events[0].id, out.ids[0]);
});
