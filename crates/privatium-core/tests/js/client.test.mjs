// Project:  Privatium™  |  File: crates/privatium-core/tests/js/client.test.mjs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-05  |  Modified: 2026-09-05
// Summary:  Browser channel framing, streaming, cancellation and origin confinement (§8.3).

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { Transport, encode, decode, endpoint, eventSource, channel, identityRefusal } from '../../assets/shell/channel.js';
import { Frame } from '../../assets/shell/session.js';

const utf8 = new TextEncoder();
function connection(limit) {
  globalThis.location = { href: 'http://192.0.2.1:8420/a/hello/', origin: 'http://192.0.2.1:8420', protocol: 'http:', host: '192.0.2.1:8420' };
  const key = new Uint8Array(32).fill(17);
  const incoming = new Frame(key, 1), outgoing = new Frame(key, 2);
  class Socket extends EventTarget {
    readyState = 1; sent = [];
    send(bytes) { this.sent.push(decode(incoming.open(bytes))); }
    close() { this.readyState = 3; this.dispatchEvent(new Event('close')); }
    reply(head, text = '') { this.dispatchEvent(new MessageEvent('message', { data: outgoing.seal(encode(head, utf8.encode(text))).buffer })); }
  }
  const socket = new Socket();
  const transport = new Transport(socket, { send: new Frame(key, 1), receive: new Frame(key, 2) }, limit);
  return { socket, transport };
}
const tick = () => new Promise(resolve => setTimeout(resolve, 0));

test('test_spec_8_3_browser_closes_at_id_budget_so_next_session_can_reconnect', async () => {
  const { socket, transport } = connection(1);
  const first = transport.fetch('/api/node'); await tick();
  socket.reply({ id: 1, kind: 'res', status: 204, headers: {} }); socket.reply({ id: 1, kind: 'end' });
  await first;
  await assert.rejects(transport.fetch('/api/node'));
  assert.equal(transport.closed, true);
  assert.throws(() => connection(65537));
});

test('test_spec_8_1_refusal_names_node_and_both_public_fingerprints_without_peer_text', () => {
  const record = { node: { id: 'abcdefgh', ed25519: Buffer.alloc(32, 1).toString('base64') } };
  const hello = JSON.stringify({ cert: Buffer.from(JSON.stringify({ node_pub: Buffer.alloc(32, 2).toString('base64'), detail: 'untrusted synthetic text' })).toString('base64') });
  const error = identityRefusal(record, hello);
  assert.equal(error.identity.node, 'abcdefgh');
  assert.match(error.identity.pinned, /^[0-9a-f]{64}$/);
  assert.match(error.identity.presented, /^[0-9a-f]{64}$/);
  assert.notEqual(error.identity.pinned, error.identity.presented);
  assert.ok(!JSON.stringify(error).includes('untrusted synthetic text'));
});

test('test_spec_7_6_missing_or_invalid_pairing_never_opens_a_socket', async () => {
  let opened = 0;
  globalThis.WebSocket = class { constructor() { opened++; } };
  for (const value of [null, '{}', '{"v":2}', 'invalid']) {
    globalThis.localStorage = { getItem: () => value };
    await assert.rejects(channel());
  }
  globalThis.localStorage = { getItem: () => { throw new Error('disabled'); } };
  await assert.rejects(channel()); assert.equal(opened, 0);
});

test('test_spec_8_3_browser_bounds_unread_body_and_refuses_future_frames', async () => {
  const { socket, transport } = connection();
  const promise = transport.fetch('/api/events'); await tick();
  socket.reply({ id: 1, kind: 'res', status: 200, headers: {} });
  const response = await promise;
  for (let i = 0; i < 17; i++) socket.reply({ id: 1, kind: 'chunk' }, 'x'.repeat(65536));
  assert.equal(socket.sent.at(-1).head.kind, 'cancel');
  await assert.rejects(response.text()); transport.close();
});

test('test_spec_10_4_browser_client_holds_exactly_one_endpoint', async () => {
  const { socket, transport } = connection();
  assert.equal(endpoint(), 'ws://192.0.2.1:8420/ws');
  for (const path of ['http://192.0.2.2:8420/', '//example.invalid/', 'https://192.0.2.1:8420/', 'javascript:alert(1)']) {
    await assert.rejects(transport.fetch(path), /origin/);
  }
  assert.equal(socket.sent.length, 0); transport.close();
});

test('test_spec_8_3_browser_decodes_bounded_frames_and_refuses_invalid_metadata', () => {
  for (const raw of [new Uint8Array(), new Uint8Array([255,255,255,255]), encode({ id: -1, kind: 'end' }), encode({ id: 2**53, kind: 'end' }), encode({ id: 1, kind: 'end', extra: true })]) assert.throws(() => decode(raw));
  const frame = decode(encode({ id: 1, kind: 'chunk' }, new Uint8Array([0,255])));
  assert.deepEqual(frame.payload, new Uint8Array([0,255]));
});

test('test_spec_8_3_browser_streams_interleaved_responses_and_cancels', async () => {
  const { socket, transport } = connection();
  const a = transport.fetch('/a/hello/api/events'), b = transport.fetch('/a/hello/api/node');
  await tick();
  assert.deepEqual(socket.sent.map(x => x.head.id), [1,2]);
  socket.reply({ id: 1, kind: 'res', status: 200, headers: {} });
  const response = await a, reader = response.body.getReader();
  socket.reply({ id: 1, kind: 'chunk' }, 'first');
  assert.equal(new TextDecoder().decode((await reader.read()).value), 'first');
  socket.reply({ id: 2, kind: 'res', status: 200, headers: {} });
  socket.reply({ id: 2, kind: 'chunk' }, 'second'); socket.reply({ id: 2, kind: 'end' });
  assert.equal(await (await b).text(), 'second');
  await reader.cancel();
  assert.equal(socket.sent.at(-1).head.kind, 'cancel');
  socket.reply({ id: 1, kind: 'chunk' }, 'already in flight');
  transport.close();
});

test('test_spec_8_3_1_browser_handoff_is_metadata_and_resume_has_no_original_body', async () => {
  const { socket, transport } = connection();
  const promise = transport.fetch('/a/hello/name', { method: 'POST', body: 'synthetic', navigation: true }); await tick();
  assert.equal(socket.sent[0].head.navigation, true);
  const handoff = '01J00000000000000000000000';
  socket.reply({ id: 1, kind: 'res', status: 200, headers: {}, handoff }); socket.reply({ id: 1, kind: 'end' });
  assert.equal((await promise).handoff, handoff);
  const resumed = transport.resume(handoff); await tick();
  assert.deepEqual(socket.sent.at(-1).head, { id: 2, kind: 'resume', handoff });
  assert.equal(socket.sent.at(-1).payload.length, 0);
  socket.reply({ id: 2, kind: 'res', status: 200, headers: {} }); socket.reply({ id: 2, kind: 'end' });
  await resumed; transport.close();
});

test('test_spec_8_3_sse_parser_preserves_events_across_utf8_and_crlf_boundaries', async () => {
  let controller;
  const stream = new ReadableStream({ start(c) { controller = c; } });
  const source = eventSource('/api/stream', async () => new Response(stream, { headers: { 'content-type':'text/event-stream' } }));
  const events = []; source.addEventListener('append', e => events.push(e.data));
  await tick();
  for (const byte of utf8.encode('event: append\r\ndata: synthetic 🐙\r\ndata: second line\r\n\r\n')) controller.enqueue(new Uint8Array([byte]));
  await tick(); assert.deepEqual(events, ['synthetic 🐙\nsecond line']); source.close();
});

test('test_spec_8_3_malformed_response_closes_all_pending_requests_without_plaintext_fallback', async () => {
  const { socket, transport } = connection();
  const promise = transport.fetch('/a/hello/api/node'); await tick();
  const refused = assert.rejects(promise, /channel/);
  socket.reply({ id: 1, kind: 'chunk' }, 'body before head');
  await refused; assert.equal(transport.closed, true);
});
