import test from 'node:test';
import assert from 'node:assert/strict';
import { estimateUsage, recordUsage, usagePace, inspectUsage, readUsageHistory } from '../src/usage-forecast.ts';
import {cursorForecastSource,comparisonGroups,forecastWindows} from '../src/quota-forecast.ts';

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

const weeklyReset=new Date(start+7*24*60*minute).toISOString();
const cursorAccount=(windows,extra={})=>({id:'cursor-user',email:'test@example.com',plan:'Premium',unified:true,warning:null,windows,...extra});

test('a connected unified account records its shared weekly pool once and keeps depleted quotas',()=>{
 const source=cursorForecastSource(cursorAccount([
  {kind:'shared',label:'Réserve commune',used:100,reset:weeklyReset},
  {kind:'totalPercentUsed',label:'Ancien compteur',used:25,reset:weeklyReset}
 ]));
 assert.equal(source.account.id,'cursor:cursor-user');
 assert.deepEqual(source.account.windows,[{id:'cursor:shared',label:'Réserve commune'}]);
 const history=recordUsage([],source.account.id,source.limits,start);
 assert.equal(history.length,1);
 assert.equal(history[0].durationMins,10080);
 assert.equal(estimateUsage(history,start).kind,'depleted');
 assert.equal(usagePace(history[0],start).hourlyBudget,0);
 assert.equal(forecastWindows(source.account,history).length,1);
 assert.equal(cursorForecastSource(cursorAccount([],{id:'other-user'})).account.id,'cursor:other-user');
});

test('unknown quota and renewal values never become zero usage or invented forecasts',()=>{
 for(const window of [
  {kind:'shared',label:'Réserve',used:null,reset:weeklyReset},
  {kind:'shared',label:'Réserve',used:0,reset:'invalid'},
  {kind:'shared',label:'Réserve',used:101,reset:weeklyReset}
 ]){
  const source=cursorForecastSource(cursorAccount([window]));
  assert.equal(recordUsage([],source.account.id,source.limits,start).length,0);
 }
 const source=cursorForecastSource(cursorAccount([{kind:'totalPercentUsed',label:'Total',used:50,reset:weeklyReset}],{unified:false}));
 assert.equal(source.limits['cursor:totalPercentUsed'].windowDurationMins,null);
});

test('legacy Cursor limits keep the actual billing cycle and their individual series',()=>{
 const source=cursorForecastSource(cursorAccount([
  {kind:'totalPercentUsed',label:'Total',used:20,start:new Date(start).toISOString(),reset:new Date(start+30*24*60*minute).toISOString()},
  {kind:'autoPercentUsed',label:'Cursor',used:40,reset:weeklyReset},
  {kind:'apiPercentUsed',label:'Autres modèles',used:60,reset:weeklyReset}
 ],{unified:false}));
 const history=recordUsage([],source.account.id,source.limits,start);
 assert.equal(history.length,3);
 assert.equal(history.find(point=>point.window==='cursor:totalPercentUsed').durationMins,43200);
 assert.equal(comparisonGroups([source.account],history).length,3);
});

test('comparison groups five-hour limits separately and compares weekly pools without double counting',()=>{
 const codex={id:'codex',name:'Codex account',service:'Codex'};
 const source=cursorForecastSource(cursorAccount([{kind:'shared',label:'Réserve commune',used:0.3,reset:weeklyReset}]));
 let history=recordUsage([],codex.id,{primary:limit(10).primary,secondary:{usedPercent:20,resetsAt:Date.parse(weeklyReset)/1000,windowDurationMins:10080}},start);
 history=recordUsage(history,source.account.id,source.limits,start);
 const groups=comparisonGroups([codex,source.account],history);
 assert.equal(groups.length,2);
 assert.deepEqual(groups.find(group=>group.key==='duration:300').items.map(item=>item.account.id),['codex']);
 assert.deepEqual(groups.find(group=>group.key==='duration:10080').items.map(item=>item.account.id),['codex','cursor:cursor-user']);
 assert.equal(comparisonGroups([codex],history).flatMap(group=>group.items).some(item=>item.account.id===source.account.id),false);
});

test('persisted shared readings survive reload alongside existing Codex history',()=>{
 const now=Date.now();
 const source=cursorForecastSource(cursorAccount([{kind:'shared',label:'Réserve commune',used:0.3,reset:new Date(now+7*24*60*minute).toISOString()}]));
 const history=recordUsage([],source.account.id,source.limits,now);
 const previous=globalThis.localStorage;
 globalThis.localStorage={getItem:()=>JSON.stringify(history)};
 try{
  // Import the reader already used by the app; no credential or provider call is involved.
  assert.equal(readUsageHistory()[0].window,'cursor:shared');
 }finally{if(previous===undefined)delete globalThis.localStorage;else globalThis.localStorage=previous}
});
