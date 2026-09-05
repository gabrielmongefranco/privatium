// Project:  Privatium™  |  File: crates/privatium-core/tests/js/pake.test.mjs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-05  |  Modified: 2026-09-05
// Summary:  The browser's SPAKE2 against the Rust vectors of tests/fixtures/pake-vectors.json
//           (spec/protocol.md §7.4.1): the password scalar, both messages, the transcript,
//           the key schedule, the confirmations, a wrong code, and refused points.

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { spake2 } from '../../assets/shell/pair.js';
import { ed25519 } from '../../assets/shell/vendor/noble/curves/ed25519.js';

const v = JSON.parse(readFileSync(new URL('../fixtures/pake-vectors.json', import.meta.url)));
const bytes = b64 => Uint8Array.from(Buffer.from(b64, 'base64'));
const b64 = value => Buffer.from(value).toString('base64');
const hex = value => Buffer.from(value).toString('hex');

test('test_spec_7_4_spake2_matches_the_checked_in_vectors', () => {
  const w = spake2.password(v.code);
  assert.equal(b64(leBytes(w)), v.w);
  const a = spake2.start('A', w, bytes(v.x)), b = spake2.start('B', w, bytes(v.y));
  assert.equal(b64(a.message), v.pA);
  assert.equal(b64(b.message), v.pB);
  const sa = a.finish(bytes(v.pB), v.device_ed25519_public, v.node_ed25519_public);
  const sb = b.finish(bytes(v.pA), v.device_ed25519_public, v.node_ed25519_public);
  for (const s of [sa, sb]) {
    assert.equal(b64(s.transcript), v.TT);
    assert.equal(b64(s.ke), v.Ke);
  }
  assert.equal(b64(sa.confirmSend), v.cA);
  assert.equal(b64(sb.confirmSend), v.cB);
  assert.ok(sa.verify(bytes(v.cB)));
  assert.ok(sb.verify(bytes(v.cA)));
  assert.ok(!sa.verify(bytes(v.cA)));
  assert.ok(!sb.verify(new Uint8Array(32)));
  assert.ok(!sb.verify('not bytes'));
  assert.throws(() => a.finish(bytes(v.pB), v.device_ed25519_public, v.node_ed25519_public), /single use/);
  assert.equal(spake2.M, v.M);
  assert.equal(spake2.N, v.N);
});

function leBytes(n) {
  const out = new Uint8Array(32);
  for (let i = 0; i < 32; i++) { out[i] = Number(n & 0xFFn); n >>= 8n; }
  return out;
}

test('test_spec_7_4_1_m_and_n_are_prime_order_points_distinct_from_g', () => {
  const order = ed25519.Point.Fn.ORDER;
  const points = [spake2.M, spake2.N].map(h => ed25519.Point.fromBytes(Buffer.from(h, 'hex'), false));
  for (const p of points) {
    assert.ok(!p.isSmallOrder());
    assert.ok(p.multiplyUnsafe(order - 1n).add(p).equals(ed25519.Point.ZERO), 'in the prime-order subgroup');
    assert.ok(!p.equals(ed25519.Point.BASE));
  }
  assert.ok(!points[0].equals(points[1]));
});

test('test_spec_7_4_a_wrong_code_derives_nothing', () => {
  const right = spake2.password(0x1234), wrong = spake2.password(0x1235);
  assert.notEqual(right, wrong);
  const a = spake2.start('A', right, bytes(v.x)), b = spake2.start('B', wrong, bytes(v.y));
  const sa = a.finish(b.message, 'QQ==', 'Qg=='), sb = b.finish(a.message, 'QQ==', 'Qg==');
  assert.ok(!sa.verify(sb.confirmSend));
  assert.ok(!sb.verify(sa.confirmSend));
  assert.notEqual(hex(sa.ke), hex(sb.ke));
  // The same code with the same secrets is deterministic; fresh secrets are not.
  assert.equal(hex(spake2.start('A', right, bytes(v.x)).message), hex(a.message));
  assert.notEqual(hex(spake2.start('A', right, new Uint8Array(64).fill(9)).message), hex(a.message));
});

test('test_spec_7_4_1_invalid_points_zero_scalars_and_bad_codes_are_refused', () => {
  const w = spake2.password(v.code);
  const identity = new Uint8Array(32); identity[0] = 1;
  const signed = identity.slice(); signed[31] |= 0x80;
  for (const bad of [identity, signed, new Uint8Array(32).fill(0xff), new Uint8Array(31), new Uint8Array(33)]) {
    assert.throws(() => spake2.start('A', w, bytes(v.x)).finish(bad, 'QQ==', 'Qg=='), /invalid key-exchange/);
  }
  assert.throws(() => spake2.start('A', w, new Uint8Array(64)), /zero scalar/);
  assert.throws(() => spake2.start('C', w, bytes(v.x)), /side/);
  assert.throws(() => spake2.start('A', 0n, bytes(v.x)), /password/);
  for (const code of [-1, 0x10000, 1.5, '1234', null]) assert.throws(() => spake2.password(code), /sixteen bits/);
  // A refusal never carries the peer's bytes.
  try { spake2.start('A', w, bytes(v.x)).finish(new Uint8Array(32).fill(0x41), 'QQ==', 'Qg=='); assert.fail('accepted'); }
  catch (e) { assert.ok(!e.message.includes('AAAA') && !e.message.includes('41')); }
});
