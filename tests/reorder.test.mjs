import test from 'node:test';
import assert from 'node:assert/strict';
import {applyAccountOrder,readAccountOrder,reorderVisibleEntries} from '../src/reorder.ts';

const accounts=[
 {id:'cursor-connected-account',service:'Cursor'},
 {id:'codex-a',service:'Codex'},
 {id:'claude-a',service:'Claude Code'},
 {id:'codex-b',service:'Codex'},
];
const ids=values=>values.map(value=>value.id);

test('Cursor moves among all accounts and keeps its place after reload',()=>{
 const moved=reorderVisibleEntries(accounts,'Tous','cursor-connected-account','codex-b',true);
 assert.deepEqual(ids(moved),['codex-a','claude-a','codex-b','cursor-connected-account']);
 const saved=JSON.parse(JSON.stringify(ids(moved)));
 assert.deepEqual(ids(applyAccountOrder(accounts,saved)),ids(moved));
 const back=reorderVisibleEntries(moved,'Tous','cursor-connected-account','claude-a',false);
 assert.deepEqual(ids(back),['codex-a','cursor-connected-account','claude-a','codex-b']);
 assert.deepEqual(ids(accounts),['cursor-connected-account','codex-a','claude-a','codex-b']);
});

test('filtered Codex moves preserve Cursor and Claude positions in the shared order',()=>{
 const moved=reorderVisibleEntries(accounts,'Codex','codex-b','codex-a',false);
 assert.deepEqual(ids(moved),['cursor-connected-account','codex-b','claude-a','codex-a']);
 assert.deepEqual(ids(applyAccountOrder(accounts,ids(moved))),ids(moved));
});

test('removed or newly connected services do not hide accounts or duplicate them',()=>{
 const saved=['deleted-account','codex-b','cursor-connected-account','codex-a'];
 assert.deepEqual(ids(applyAccountOrder(accounts,saved)),['codex-b','cursor-connected-account','codex-a','claude-a']);
 const disconnected=accounts.filter(account=>account.service!=='Cursor');
 assert.deepEqual(ids(applyAccountOrder(disconnected,saved)),['codex-b','codex-a','claude-a']);
 assert.equal(reorderVisibleEntries(accounts,'Tous','missing','codex-a',true),accounts);
 assert.equal(reorderVisibleEntries(accounts,'Tous','codex-a','codex-a',true),accounts);
});

test('stored ordering is bounded and rejects corrupt values without losing accounts',()=>{
 const previous=globalThis.localStorage;
 try{
  globalThis.localStorage={getItem:()=>JSON.stringify(['codex-a',null,4,'codex-a','cursor-connected-account'])};
  assert.deepEqual(readAccountOrder(),['codex-a','cursor-connected-account']);
  globalThis.localStorage={getItem:()=>'{broken'};
  assert.deepEqual(readAccountOrder(),[]);
  assert.deepEqual(ids(applyAccountOrder(accounts,readAccountOrder())),ids(accounts));
 }finally{if(previous===undefined)delete globalThis.localStorage;else globalThis.localStorage=previous}
});
