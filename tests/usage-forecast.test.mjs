import test from 'node:test';
import assert from 'node:assert/strict';
import { estimateUsage, recordUsage, usagePace, inspectUsage } from '../src/usage-forecast.ts';

const minute=60_000;
const start=1_750_000_000_000;
const reset=Math.floor((start+5*60*minute)/1000);
const limit=(used,resetsAt=reset)=>({primary:{usedPercent:used,windowDurationMins:300,resetsAt}});

test('a first depleted reading shows zero budget and a regular-use reference without inventing history',()=>{
 const now=start+150*minute;
 const history=recordUsage([],'new-account',limit(100),now);
 assert.equal(history.length,1);
 assert.equal(estimateUsage(history,now).kind,'depleted');
 const pace=usagePace(history[0],now);
 assert.equal(pace.start,start);
 assert.equal(pace.targetRemaining,50);
 assert.equal(pace.remaining,0);
 assert.equal(pace.hourlyBudget,0);
 assert.equal(pace.gap,-50);
});

test('pacing divides the actual remaining quota over time to reset, independently of trend history',()=>{
 const now=start+150*minute;
 const [point]=recordUsage([],'a',limit(25),now);
 const pace=usagePace(point,now);
 assert.equal(pace.targetRemaining,50);
 assert.equal(pace.hourlyBudget,30);
 assert.equal(pace.gap,25);
 assert.equal(estimateUsage([point],now).kind,'insufficient');
 assert.equal(usagePace({...point,durationMins:null},now),null);
 assert.equal(usagePace({...point,durationMins:-1},now),null);
 assert.equal(usagePace(point,reset*1000),null);
 assert.equal(usagePace(point,start-minute),null);
});

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

test('chart inspection distinguishes measured readings from future estimates',()=>{
 let history=recordUsage([],'a',limit(10),start);
 history=recordUsage(history,'a',limit(20),start+15*minute);
 const past=inspectUsage(history,start+10*minute,start+15*minute);
 assert.equal(past.point.used,20);
 assert.equal(past.point.at,start+15*minute);
 assert.equal(past.predictedRemaining,null);
 const future=inspectUsage(history,start+30*minute,start+15*minute);
 assert.equal(future.point.used,20);
 assert.equal(future.predictedRemaining,70);
 assert.equal(inspectUsage(history,reset*1000+minute,start+15*minute).predictedRemaining,null);
 assert.equal(inspectUsage([history[0]],start+30*minute,start).predictedRemaining,null);
 assert.equal(inspectUsage(history,start+30*minute,start+60*minute).predictedRemaining,null);
 assert.equal(inspectUsage([],start,start),null);
});
