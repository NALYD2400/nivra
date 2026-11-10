import test from 'node:test';
import assert from 'node:assert/strict';
import {upsertAntigravityAccount} from '../src/antigravity-accounts.ts';

const original={id:'google-a',email:'a@example.com',project:'',credential_ref:'old-a',project_error:'missing'};
const other={id:'google-b',email:'b@example.com',project:'b',credential_ref:'old-b'};

test('a repeat Google login replaces only its verified account and clears stale project errors',()=>{
 const replacement={...original,credential_ref:'new-a',project:'ready',project_error:null};
 const {next,previous}=upsertAntigravityAccount([original,other],replacement);
 assert.deepEqual(next,[replacement,other]);
 assert.equal(previous.credential_ref,'old-a');
 assert.equal(original.credential_ref,'old-a');
 assert.equal(next[1].credential_ref,'old-b');
});

test('a different verified identity cannot overwrite credentials by matching an email',()=>{
 const account={...original,id:'different-google-id',credential_ref:'new-identity'};
 const {next,previous}=upsertAntigravityAccount([original,other],account);
 assert.equal(previous,undefined);
 assert.deepEqual(next,[original,other,account]);
});
