import test from 'node:test';
import assert from 'node:assert/strict';
import { matchAntigravityUsage, parseAntigravityUsages } from '../src/antigravity-usage.ts';

test('matches a live account by its reported identity, ignoring case',()=>{
 const samples=[{email:'alice@example.com',models:[]},{email:'bob@example.com',models:[]}];
 assert.equal(matchAntigravityUsage(' ALICE@example.com ',samples),samples[0]);
 assert.equal(matchAntigravityUsage('eve@example.com',samples),null);
 assert.equal(matchAntigravityUsage(undefined,samples),null);
});

test('refuses an ambiguous identity instead of displaying another session’s quota',()=>{
 const samples=[{email:'alice@example.com',models:[]},{email:'ALICE@example.com',models:[]}];
 assert.equal(matchAntigravityUsage('alice@example.com',samples),null);
});

test('discards malformed local-server data',()=>{
 const valid={email:'alice@example.com',models:[{label:'model',remaining:0.5}]};
 assert.deepEqual(parseAntigravityUsages([valid,{email:'bob@example.com',models:[{label:'x',remaining:'bad'}]},null]),[valid,{email:'bob@example.com',models:[]}]);
 assert.deepEqual(parseAntigravityUsages({email:'alice@example.com'}),[]);
});

test('keeps valid model quotas without fabricating invalid values',()=>{
 const sample={email:'alice@example.com',plan:'Pro',models:[{label:'valid',remaining:0.25,reset:'2026-11-10T12:00:00Z'},{label:'invalid',remaining:2},{label:'negative',remaining:-0.1},{label:'nan',remaining:NaN},{label:'valid without reset',remaining:0,reset:3}]};
 assert.deepEqual(parseAntigravityUsages([sample]),[{email:'alice@example.com',plan:'Pro',models:[sample.models[0],{label:'valid without reset',remaining:0}]}]);
});

test('preserves only valid 5-hour and weekly quota windows',()=>{
 const sample={email:'alice@example.com',models:[],windows:[{group:'Gemini',window:'5h',remaining:0},{group:'Gemini',window:'weekly',remaining:0.75,reset:'2030-01-01T00:00:00Z'},{group:'Gemini',window:'weekly',remaining:2},{group:'Gemini',window:'daily',remaining:0.5}]};
 assert.deepEqual(parseAntigravityUsages([sample]),[{email:sample.email,models:[],windows:sample.windows.slice(0,2)}]);
});
