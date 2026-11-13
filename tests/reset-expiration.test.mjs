import test from 'node:test';
import assert from 'node:assert/strict';
import {expiresSoon} from '../src/reset-expiration.ts';

test('only future expirations within three days trigger the warning',()=>{
 const now=1700000000000, seconds=now/1000;
 assert.equal(expiresSoon(seconds+3*86400,now),true);
 assert.equal(expiresSoon(seconds+3*86400+1,now),false);
 assert.equal(expiresSoon(seconds,now),false);
 assert.equal(expiresSoon(null,now),false);
 assert.equal(expiresSoon(Number.NaN,now),false);
});
