// Project: Privatium™ | File: crates/privatium-core/tests/js/sketch.test.mjs
// Authors: Gabriel Mongefranco (@gabrielmongefranco)
// Created: 2026-09-06 | Modified: 2026-09-06
// Summary: Stroke erasing respects segment edges and rejects malformed geometry.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { hitsStroke } from '../../../../apps/sketch/web/strokes.js';
test('eraser hits segment interiors, endpoints and width boundary without mutation', () => {
  const stroke = { width: 4, points: [[0, 0], [100, 0]] };
  const before = JSON.stringify(stroke);
  assert.equal(hitsStroke(stroke, 50, 8), true);
  assert.equal(hitsStroke(stroke, 50, 8.01), false);
  assert.equal(hitsStroke(stroke, -8, 0), true);
  assert.equal(hitsStroke(stroke, 109, 0), false);
  assert.equal(JSON.stringify(stroke), before);
});
test('single points and malformed strokes are handled safely', () => {
  assert.equal(hitsStroke({width: 4, points: [[2, 2]]}, 2, 2), true);
  for (const stroke of [null, {}, {width: 4, points: []}, {width: -1, points: [[0,0]]}, {width:4, points:[[NaN,0]]}]) {
    assert.equal(hitsStroke(stroke, 0, 0), false);
  }
  assert.equal(hitsStroke({width:4, points:[[0,0]]}, Infinity, 0), false);
});

import { SketchHistory } from '../../../../apps/sketch/web/history.js';
let nextId = 0;
const makeHistory = write => new SketchHistory(write, () => `restored-${nextId++}`);
const stroke = (id, color = '#00274C') => ({op:'put', tbl:'stroke', id, d:{points:[[1,1],[2,2]], color, width:3}});
test('undo restores painting order and reverses new sketch, including replay', async () => {
 const writes=[]; const m=makeHistory(async e=>{writes.push(e);});
 await m.change([stroke('a'),stroke('b','#FFFFFF')]);
 await m.change([{op:'del',tbl:'stroke',id:'a'}]); await m.undo();
 assert.deepEqual(m.entries().map(([,s])=>s.color),['#00274C','#FFFFFF']);
 await m.change(m.entries().map(([id])=>({op:'del',tbl:'stroke',id})));
 assert.equal(m.strokes.size,0); await m.undo();
 const replay=makeHistory(async()=>{}); writes.flat().forEach(e=>replay.apply(e));
 assert.deepEqual(replay.entries(),m.entries());
});
test('failed writes and remote edits preserve undo and visible strokes', async()=>{
 let fail=false; const m=makeHistory(async()=>{if(fail)throw new Error('offline');});
 await m.change([stroke('a')]); fail=true;
 await assert.rejects(m.change([stroke('b')])); await assert.rejects(m.undo());
 assert.equal(m.strokes.has('b'),false); assert.equal(m.undoStack.length,1);
 fail=false; m.apply(stroke('a','#FFFFFF'));
 await assert.rejects(m.undo(),/another window/);
 assert.equal(m.strokes.get('a').color,'#FFFFFF');
});
test('history is bounded and simultaneous actions are refused',async()=>{
 const m=makeHistory(async()=>{});
 for(let i=0;i<51;i++)await m.change([stroke(String(i))]);
 assert.equal(m.undoStack.length,50);
 let done; m.write=()=>new Promise(resolve=>{done=resolve;});
 const pending=m.change([stroke('pending')]);
 await assert.rejects(m.undo(),/Wait/); await assert.rejects(m.change([stroke('other')]),/Wait/);
 done(); await pending;
});

test('undo never reuses tombstoned IDs and earlier undo follows restored strokes',async()=>{
 const deleted=new Set();
 const m=makeHistory(async events=>{
  for(const ev of events) {
   if(ev.op==='del')deleted.add(ev.id);
   else assert.equal(deleted.has(ev.id),false,'a deleted ID cannot be reused');
  }
 });
 await m.change([stroke('original')]);
 await m.change([{op:'del',tbl:'stroke',id:'original'}]);
 await m.undo();
 assert.equal(m.strokes.has('original'),false);
 assert.equal(m.strokes.size,1);
 await m.undo();
 assert.equal(m.strokes.size,0);
});
