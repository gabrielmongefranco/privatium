// Project:  Privatium™  |  File: crates/privatium-core/tests/js/navigation.test.mjs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-05  |  Modified: 2026-09-05
// Summary:  Encrypted HTMX dispatch and metadata-only document handoffs (§8.3.1).

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { bridgeHtmx, saveHandoff, takeHandoff, resourceIntegrity } from '../../assets/shell/client.js';
import { createHash } from 'node:crypto';
import { storage } from './harness.mjs';

test('test_spec_8_3_integrity_uses_authenticated_bytes_and_refuses_unpinned_remote_code', async () => {
  globalThis.location = { origin: 'http://192.0.2.1' };
  const bytes = 'synthetic source', expected = 'sha256-' + createHash('sha256').update(bytes).digest('base64');
  let fetched = 0;
  const fetcher = async () => { fetched++; return new Response(bytes); };
  assert.equal(await resourceIntegrity(new URL('http://192.0.2.1/static/app.js'), 'untrusted hash', fetcher), expected);
  assert.equal(fetched, 1);
  assert.equal(await resourceIntegrity(new URL('https://example.invalid/app.js'), expected, fetcher), expected);
  await assert.rejects(resourceIntegrity(new URL('https://example.invalid/app.js'), '', fetcher));
  await assert.rejects(resourceIntegrity(new URL('https://example.invalid/app.js'), 'sha256-a', fetcher));
  await assert.rejects(resourceIntegrity(new URL('javascript:alert(1)'), expected, fetcher));
  assert.equal(fetched, 1);
});

test('test_spec_8_3_1_browser_stores_only_reference_and_consumes_before_attach', () => {
  globalThis.sessionStorage = storage();
  globalThis.location = { href: 'http://192.0.2.1/a/hello/', origin: 'http://192.0.2.1' };
  const id = '01J00000000000000000000000';
  saveHandoff(id, '/a/hello/name', 'synthetic');
  assert.deepEqual([...sessionStorage.map.values()].map(JSON.parse), [{ handoff: id, path: '/a/hello/name', node: 'synthetic' }]);
  assert.equal(takeHandoff('/a/hello/name', 'synthetic'), id);
  assert.equal(sessionStorage.length, 0);
  assert.equal(takeHandoff('/a/hello/name', 'synthetic'), null);
  saveHandoff(id, '/a/hello/name', 'synthetic');
  assert.throws(() => takeHandoff('/different', 'synthetic'));
  assert.equal(sessionStorage.length, 0);
});

test('test_spec_8_3_htmx_keeps_its_lifecycle_and_never_sends_plaintext', async () => {
  globalThis.location = { href: 'http://192.0.2.1/a/hello/', origin: 'http://192.0.2.1' };
  const xhr = new EventTarget(); let loaded = false, native = false;
  xhr.send = () => { native = true; };
  xhr.onload = () => { loaded = true; };
  xhr.onerror = () => assert.fail('channel should answer');
  let captured;
  bridgeHtmx({ xhr, pathInfo: { finalRequestPath: '/a/hello/name' }, requestConfig: { verb: 'post', headers: { 'HX-Request': 'true' } } }, async (path, init) => {
    captured = { path, init }; return new Response('<p>synthetic</p>', { headers: { 'HX-Trigger': 'saved' } });
  });
  await xhr.send('name=synthetic');
  assert.equal(native, false); assert.equal(loaded, true);
  assert.equal(captured.init.body, 'name=synthetic');
  assert.equal(xhr.status, 200); assert.equal(xhr.response, '<p>synthetic</p>');
  assert.equal(xhr.getResponseHeader('hx-trigger'), 'saved');
});
