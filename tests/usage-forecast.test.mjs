import test from 'node:test';
import assert from 'node:assert/strict';
import { estimateUsage, recordUsage } from '../src/usage-forecast.ts';

const minute=60_000;
const start=1_750_000_000_000;
const reset=Math.floor((start+5*60*minute)/1000);
const limit=(used,resetsAt=reset)=>({primary:{usedPercent:used,windowDurationMins:300,resetsAt}});

test('calculates exhaustion from actual readings in the same window',()=>{
  let history=recordUsage([],'a',limit(10),start);
  assert.equal(estimateUsage(history,start).kind,'insufficient');
  history=recordUsage(history,'a',limit(20),start+15*minute);
  const result=estimateUsage(history,start+15*minute);
  assert.equal(result.kind,'projected');
  assert.equal(result.at,start+135*minute);
});

test('does not invent an exhaustion date for flat, stale or slow use',()=>{
  const first=recordUsage([],'a',limit(10),start);
  assert.equal(estimateUsage(recordUsage(first,'a',limit(10),start+15*minute),start+15*minute).kind,'unchanged');
  assert.equal(estimateUsage(recordUsage(first,'a',limit(11),start+15*minute),start+15*minute).kind,'after-reset');
  assert.equal(estimateUsage(first,start+25*minute).kind,'stale');
  assert.equal(estimateUsage(first,reset*1000).kind,'stale');
});

test('records windows and resets independently and rejects invalid values',()=>{
  const first=recordUsage([],'a',limit(10),start);
  assert.equal(recordUsage(first,'a',limit(11),start+minute).length,1);
  const resetNext=reset+18000;
  const next=recordUsage(first,'a',limit(2,resetNext),start+minute);
  assert.equal(next.length,2);
  assert.equal(estimateUsage(next.filter(p=>p.resetAt===resetNext),start+minute).kind,'insufficient');
  assert.equal(recordUsage([],'a',limit(101),start).length,0);
  assert.equal(recordUsage([],'a',limit(50,Math.floor(start/1000)-1),start).length,0);
  assert.equal(estimateUsage(recordUsage([],'a',limit(100),start),start).kind,'depleted');
});

test('does not extrapolate after a substantial correction within a period',()=>{
  const first=recordUsage([],'a',limit(10),start);
  const second=recordUsage(first,'a',limit(20),start+15*minute);
  const corrected=recordUsage(second,'a',limit(15),start+30*minute);
  assert.equal(estimateUsage(corrected,start+30*minute).kind,'insufficient');
});
