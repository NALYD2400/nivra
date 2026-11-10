import type {UsageLimits,UsagePoint,UsageWindow} from './usage-forecast';

export type ForecastAccount={id:string;name:string;service:'Codex'|'Claude Code'|'Cursor'|'xAI / Grok';windows?:{id:UsageWindow;label:string}[]};
export type CursorUsage={id:string;email:string;plan:string|null;unified:boolean|null;windows:{kind:string;label:string;used:number|null;reset:string|null;start?:string|null;durationMins?:number|null}[];warning:string|null};

// One source per account, even when the same pool appears on two service pages.
export function cursorForecastSource(data:CursorUsage){
 const limits:UsageLimits={};
 const windows:NonNullable<ForecastAccount['windows']>=[];
 for(const window of data.windows){
  if(!/^[a-zA-Z0-9_-]{1,80}$/.test(window.kind))continue;
  if(data.unified===true&&window.kind!=='shared')continue;
  const id:UsageWindow=`cursor:${window.kind}`;
  if(windows.some(window=>window.id===id))continue;
  windows.push({id,label:window.label});
  if(window.used===null||!Number.isFinite(window.used)||window.used<0||window.used>100)continue;
  const end=window.reset?Date.parse(window.reset):NaN;
  const start=window.start?Date.parse(window.start):NaN;
  const weekly=window.kind==='shared'&&data.unified===true||window.kind==='grok-bot';
  const duration=weekly?10080:window.durationMins??(Number.isFinite(start)&&end>start?(end-start)/60_000:null);
  limits[id]={usedPercent:window.used,resetsAt:Number.isFinite(end)?Math.floor(end/1000):null,windowDurationMins:duration&&Number.isFinite(duration)&&duration>0?duration:null};
 }
 const account:ForecastAccount={id:`cursor:${data.id}`,name:data.unified?`${data.email} · Cursor / Grok`:data.email,service:'Cursor',windows};
 return {account,limits};
}

export function forecastWindows(account:ForecastAccount,history:UsagePoint[]){
 if(account.windows)return account.windows;
 const recorded=[...new Set(history.filter(point=>point.accountId===account.id).map(point=>point.window))];
 return (recorded.length?recorded:['primary'] as UsageWindow[]).map(id=>({id,label:id==='primary'?'Limite principale':'Limite secondaire'}));
}

// Compare equal periods together: a weekly pool must not become a five-hour line.
export function comparisonGroups(accounts:ForecastAccount[],history:UsagePoint[]){
 const groups=new Map<string,{key:string;label:string;items:{account:ForecastAccount;window:UsageWindow}[]}>();
 for(const account of accounts){
  if(account.service==='Claude Code')continue;
  for(const window of forecastWindows(account,history)){
   const points=history.filter(point=>point.accountId===account.id&&point.window===window.id);
   const last=points.reduce<UsagePoint|undefined>((latest,point)=>!latest||point.at>latest.at?point:latest,undefined);
   const duration=last?.durationMins;
   const separate=window.id.startsWith('cursor:')&&window.id!=='cursor:shared'?`:${window.id}`:'';
   const key=duration&&Number.isFinite(duration)&&duration>0?`duration:${duration}${separate}`:`${account.service}:${window.id}`;
   let group=groups.get(key);
   if(!group){group={key,label:window.label,items:[]};groups.set(key,group)}
   // Distinct limits of one account remain separate, even for equal durations.
   if(group.items.some(item=>item.account.id===account.id)){
    const extraKey=`${key}:${window.id}`;
    let extra=groups.get(extraKey);if(!extra){extra={key:extraKey,label:window.label,items:[]};groups.set(extraKey,extra)}
    extra.items.push({account,window:window.id});
   }else group.items.push({account,window:window.id});
  }
 }
 return [...groups.values()];
}
