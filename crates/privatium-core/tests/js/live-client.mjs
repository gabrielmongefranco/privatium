// Project:  Privatium™  |  File: crates/privatium-core/tests/js/live-client.mjs
// Authors:  Gabriel Mongefranco (@gabrielmongefranco)
// Created:  2026-09-05  |  Modified: 2026-09-05
// Summary:  Browser modules against a live core, driven by tests/channel.rs (§8.3).

import assert from 'node:assert/strict';
import { pair } from '../../assets/shell/pair.js';
import { channel } from '../../assets/shell/channel.js';

const deadline = setTimeout(() => { process.stderr.write('Live client timed out.\n'); process.exit(1); }, 15000);
try {
  let input = '';
  for await (const bytes of process.stdin) input += bytes;
  const config = JSON.parse(input); input = '';
  globalThis.location = new URL(config.origin + '/a/hello/');
  const values = new Map();
  globalThis.localStorage = { getItem: key => values.get(key) ?? null, setItem: (key, value) => values.set(key, value), removeItem: key => values.delete(key) };
  const socket = new WebSocket(config.origin.replace('http:', 'ws:') + '/ws/pair');
  await new Promise((resolve, reject) => { socket.onopen = resolve; socket.onerror = reject; });
  const paired = await pair(socket, config.code);
  config.code = '';
  let client = await channel();
  const node = await (await client.fetch('/a/hello/api/node')).json();
  assert.equal(node.dev, paired.dev);
  const response = await client.fetch('/a/hello/');
  assert.equal(response.status, 200);
  assert.ok((await response.text()).includes('<h1>'));
  const post = await client.fetch('/a/hello/api/events', { method: 'POST', headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ events: [{ op: 'put', tbl: 'notes', id: '01J00000000000000000000002', d: { text: 'Synthetic live client' } }] }), navigation: true });
  assert.ok(post.handoff); client.close();
  client = await channel();
  assert.equal((await (await client.resume(post.handoff)).json()).appended, 1);
  assert.equal((await client.resume(post.handoff)).status, 409);
  const edit = await (await client.fetch('/a/hello/edit')).text();
  const csrf = edit.match(/name="_csrf" value="([^"]+)"/)[1];
  const form = await client.fetch('/a/hello/name', { method: 'POST', body: new URLSearchParams({ _csrf: csrf, display_name: '' }), navigation: true });
  assert.ok(form.handoff); client.close(); client = await channel();
  const validation = await client.resume(form.handoff);
  assert.equal(validation.headers.get('content-type'), 'text/html; charset=utf-8');
  assert.ok((await validation.text()).includes('Please enter a name.'));
  client.close(); values.clear();
  process.stdout.write('browser-live: passed\n');
} catch (error) {
  process.stderr.write('Live client failed: ' + (error.code || error.name || 'Error') + '\n'); process.exitCode = 1;
} finally { clearTimeout(deadline); }
