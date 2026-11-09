export type UsageWindow = 'primary'|'secondary';
export type UsagePoint = { accountId:string; window:UsageWindow; at:number; used:number; resetAt:number; durationMins:number|null };
export type ForecastLimit = {usedPercent:number;windowDurationMins:number|null;resetsAt:number|null};
export type UsageLimits = {primary?:ForecastLimit|null;secondary?:ForecastLimit|null};
export type Estimate = {kind:'insufficient'|'stale'|'unchanged'|'after-reset'|'depleted'|'projected';at?:number;rate?:number};

const RETENTION_MS=14*24*60*60*1000;
const MIN_SAMPLE_MS=2*60*1000;
const MIN_TREND_MS=15*60*1000;
const MAX_POINTS=18000;

export function readUsageHistory():UsagePoint[]{
 try{
  const value:unknown=JSON.parse(localStorage.getItem('nivra-usage-history-v1')||'[]');
  if(!Array.isArray(value))return [];
  const cutoff=Date.now()-RETENTION_MS;
  return value.filter((p):p is UsagePoint=>Boolean(p&&typeof p.accountId==='string'&&(p.window==='primary'||p.window==='secondary')&&Number.isFinite(p.at)&&p.at>=cutoff&&p.at<=Date.now()+60000&&Number.isFinite(p.used)&&p.used>=0&&p.used<=100&&Number.isFinite(p.resetAt))).slice(-MAX_POINTS);
 }catch{return []}
}

export function recordUsage(history:UsagePoint[],accountId:string,limits:UsageLimits|undefined,now:number):UsagePoint[]{
 if(!limits)return history;
 const additions:UsagePoint[]=[];
 for(const window of ['primary','secondary'] as const){
  const limit=limits[window];
  if(!limit||!Number.isFinite(limit.usedPercent)||limit.usedPercent<0||limit.usedPercent>100||!limit.resetsAt||!Number.isFinite(limit.resetsAt)||limit.resetsAt*1000<=now)continue;
  let previous:UsagePoint|undefined;
  for(let i=history.length-1;i>=0;i--){const point=history[i];if(point.accountId===accountId&&point.window===window&&point.resetAt===limit.resetsAt){previous=point;break}}
  if(previous&&now-previous.at<MIN_SAMPLE_MS)continue;
  additions.push({accountId,window,at:now,used:limit.usedPercent,resetAt:limit.resetsAt,durationMins:limit.windowDurationMins});
 }
 if(additions.length===0)return history;
 return [...history.filter(p=>p.at>=now-RETENTION_MS),...additions].slice(-MAX_POINTS);
}

export function estimateUsage(points:UsagePoint[],now:number):Estimate{
 if(points.length===0)return {kind:'insufficient'};
 const latest=points[points.length-1];
 if(latest.resetAt*1000<=now)return {kind:'stale'};
 if(now-latest.at>20*60*1000)return {kind:'stale'};
 if(latest.used>=100)return {kind:'depleted'};
 const earliest=points.find(point=>point.resetAt===latest.resetAt);
 if(!earliest||latest.at-earliest.at<MIN_TREND_MS)return {kind:'insufficient'};
 if(points.some((point,index)=>index>0&&point.used<points[index-1].used-1))return {kind:'insufficient'};
 const used=latest.used-earliest.used;
 if(used<0)return {kind:'insufficient'};
 if(used<0.1)return {kind:'unchanged'};
 const rate=used/(latest.at-earliest.at);
 const at=latest.at+(100-latest.used)/rate;
 if(!Number.isFinite(at))return {kind:'insufficient'};
 if(at>=latest.resetAt*1000)return {kind:'after-reset',rate};
 return {kind:'projected',at,rate};
}
