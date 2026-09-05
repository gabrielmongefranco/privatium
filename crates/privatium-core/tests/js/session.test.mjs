// Project:  Privatium™  |  File: crates/privatium-core/tests/js/session.test.mjs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-05  |  Modified: 2026-09-05
// Summary:  Rust/browser session interoperability and failure-closed crypto under
//           spec/protocol.md §8, including operation without crypto.subtle.

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { derive, Frame, clientHandshake } from '../../assets/shell/session.js';
import { x25519 } from '../../assets/shell/vendor/noble/curves/ed25519.js';

const vector = JSON.parse(readFileSync(new URL('../fixtures/session-vectors.json', import.meta.url)));
const bytes = b64 => Uint8Array.from(Buffer.from(b64, 'base64'));
const b64 = value => Buffer.from(value).toString('base64');
const fixed = n => new Uint8Array(32).fill(n);
const encoder = new TextEncoder();
const pins = () => ({ id: 'as3nn9tm', cluster: bytes(vector.handshake.cluster_public),
  x25519: bytes(vector.handshake.node_static_public) });
const keys = () => derive(fixed(1), x25519.getPublicKey(fixed(2)), fixed(3), x25519.getPublicKey(fixed(4)), vector.node_id, vector.device_id);

test('test_spec_8_key_schedule_and_bidirectional_frames_match_rust_vectors', () => {
  const key = keys();
  for (const [direction, name] of [[1, 'c2s'], [2, 's2c']]) {
    assert.equal(b64(key[name]), vector[name].key);
    const send = new Frame(key[name], direction), receive = new Frame(key[name], direction);
    for (const frame of vector[name].frames) {
      assert.equal(b64(send.seal(bytes(frame.plaintext))), frame.ciphertext);
      assert.equal(b64(receive.open(bytes(frame.ciphertext))), frame.plaintext);
    }
  }
});

test('test_spec_8_tamper_replay_truncation_and_order_fail_permanently', () => {
  const key = keys().c2s, first = bytes(vector.c2s.frames[0].ciphertext);
  for (const bad of [new Uint8Array(), first.slice(1), bytes(vector.c2s.frames[1].ciphertext)]) {
    const frame = new Frame(key, 1);
    assert.throws(() => frame.open(bad));
    assert.throws(() => frame.open(first), /closed/);
  }
  const tampered = first.slice(); tampered[0] ^= 1;
  assert.throws(() => new Frame(key, 1).open(tampered));
  assert.throws(() => new Frame(key, 2).open(first));
  const receive = new Frame(key, 1);
  receive.open(first);
  assert.throws(() => receive.open(first));
});

test('test_spec_8_counter_limit_cannot_be_raised_and_key_input_is_copied', () => {
  const key = keys().c2s, original = key.slice();
  const send = new Frame(key, 1, 2), receive = new Frame(original, 1, 2);
  key.fill(0);
  const first = send.seal(new Uint8Array()), second = send.seal(new Uint8Array());
  assert.notDeepEqual(first, second);
  receive.open(first); receive.open(second);
  assert.equal(send.closed, true);
  assert.equal(receive.closed, true);
  assert.throws(() => send.seal(new Uint8Array()), /closed/);
  assert.throws(() => receive.open(first), /closed/);
  for (const limit of [0, -1, NaN, Infinity, 2 ** 32 + 1, 1.5]) assert.throws(() => new Frame(original, 1, limit));
  for (const direction of [0, 3, '1']) assert.throws(() => new Frame(original, direction));
});

test('test_spec_8_low_order_keys_and_invalid_ids_are_refused', () => {
  for (const zero of [new Uint8Array(32), Uint8Array.from([1, ...new Uint8Array(31)])]) {
    assert.throws(() => derive(fixed(1), zero, fixed(3), x25519.getPublicKey(fixed(4)), vector.node_id, vector.device_id));
    assert.throws(() => derive(fixed(1), x25519.getPublicKey(fixed(2)), fixed(3), zero, vector.node_id, vector.device_id));
  }
  for (const id of ['', '../bad', 'AAAAAAAA', 'b3nn8t2q\n', null]) assert.throws(() => derive(fixed(1), x25519.getPublicKey(fixed(2)), fixed(3), x25519.getPublicKey(fixed(4)), id, vector.device_id));
});

function start() {
  const descriptor = Object.getOwnPropertyDescriptor(globalThis, 'crypto');
  // Simulate plain HTTP: only getRandomValues exists, never subtle.
  Object.defineProperty(globalThis, 'crypto', { configurable: true, value: { getRandomValues: a => { a.fill(3); return a; } } });
  try { return clientHandshake(vector.device_id, fixed(1), pins()); }
  finally { Object.defineProperty(globalThis, 'crypto', descriptor); }
}

test('test_spec_8_client_handshake_matches_rust_without_crypto_subtle', () => {
  const client = start();
  assert.deepEqual(JSON.parse(client.hello), JSON.parse(vector.handshake.client_hello));
  // Rust fixture deliberately uses sorted fields; confirmation binds actual bytes.
  const result = client.finish(vector.handshake.node_hello, Date.parse(vector.handshake.now));
  const fixtureHello = JSON.parse(vector.handshake.client_hello);
  assert.equal(client.hello, JSON.stringify(fixtureHello));
  assert.equal(b64(result.confirm), vector.handshake.confirm);
  assert.throws(() => client.finish(vector.handshake.node_hello), /closed/);
  assert.ok(result.send.seal(encoder.encode('synthetic request')).length > 16);
});

test('test_spec_8_1_invalid_certificates_never_produce_a_confirm', () => {
  const hello = JSON.parse(vector.handshake.node_hello);
  for (const change of [h => { h.cert = ''; }, h => { h.id = '00000000'; },
    h => { const cert = JSON.parse(Buffer.from(h.cert, 'base64')); cert.sig = b64(new Uint8Array(64)); h.cert = b64(encoder.encode(JSON.stringify(cert))); }]) {
    const bad = structuredClone(hello); change(bad);
    const client = start();
    assert.throws(() => client.finish(JSON.stringify(bad), Date.parse(vector.handshake.now)), /pinned/);
    assert.throws(() => client.finish(vector.handshake.node_hello), /closed/);
  }
  assert.throws(() => start().finish(vector.handshake.node_hello, Date.parse('2027-03-03T12:00:00.000Z')), /pinned/);
  assert.throws(() => start().finish(vector.handshake.node_hello, NaN), /pinned/);
});

test('test_spec_8_3_malformed_or_incompatible_node_hello_is_refused', () => {
  for (const hello of ['', '{}', '[]', 'x'.repeat(8193), JSON.stringify({v: 2}),
    JSON.stringify({...JSON.parse(vector.handshake.node_hello), e: b64(new Uint8Array(32))})]) {
    const client = start();
    assert.throws(() => client.finish(hello, Date.parse(vector.handshake.now)));
    assert.throws(() => client.finish(vector.handshake.node_hello), /closed/);
  }
});

test('test_spec_8_noble_hashes_and_relative_import_closure_match_provenance', () => {
  const base = new URL('../../assets/shell/vendor/noble/', import.meta.url);
  const record = readFileSync(new URL('VENDOR.md', base), 'utf8');
  const files = new Map([...record.matchAll(/\| `([^`]+\.js)` \| `([a-f0-9]{64})` \| `([a-f0-9]{64})` \|/g)].map(m => [m[1], m[3]]));
  assert.equal(files.size, 31);
  for (const [path, expected] of files) {
    const url = new URL(path, base), bytes = readFileSync(url);
    assert.equal(createHash('sha256').update(bytes).digest('hex'), expected, path);
    for (const [, specifier] of bytes.toString('utf8').matchAll(/(?:from\s*|import\s*)['"]([^'"]+)['"]/g)) {
      assert.ok(specifier.startsWith('.'), specifier);
      const dependency = new URL(specifier, url);
      assert.ok(dependency.href.startsWith(base.href), specifier);
      assert.ok(files.has(dependency.href.slice(base.href.length)), specifier);
    }
  }
});

test('test_spec_8_errors_do_not_echo_untrusted_input_or_secret_state', () => {
  const marker = 'synthetic-private-marker';
  for (const value of [marker, JSON.stringify({v:1,id:marker,cert:marker})]) {
    assert.throws(() => start().finish(value), e => !e.message.includes(marker));
  }
  const frame = new Frame(keys().c2s, 1);
  assert.equal(JSON.stringify(frame), '{}');
  frame.close();
  assert.throws(() => frame.seal(new Uint8Array()), /closed/);
});
