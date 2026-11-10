export type AntigravityUsage = {email?:string;plan?:string;models:{label:string;remaining:number;reset?:string}[];windows?:{group:string;window:'5h'|'weekly';remaining:number;reset?:string}[]};

export function parseAntigravityUsages(value:unknown):AntigravityUsage[] {
 if(!Array.isArray(value))return [];
 return value.flatMap((entry:unknown):AntigravityUsage[]=>{
  if(!entry||typeof entry!=='object')return [];
  const data=entry as {email?:unknown;plan?:unknown;models?:unknown;windows?:unknown};
  if(typeof data.email!=='string'||!data.email.includes('@')||data.email.length>320||!Array.isArray(data.models))return [];
  const models=data.models.flatMap((model:unknown):AntigravityUsage['models']=>{
   if(!model||typeof model!=='object')return [];
   const item=model as {label?:unknown;remaining?:unknown;reset?:unknown};
   if(typeof item.label!=='string'||!item.label.trim()||item.label.length>300||typeof item.remaining!=='number'||!Number.isFinite(item.remaining)||item.remaining<0||item.remaining>1)return [];
   return [{label:item.label,remaining:item.remaining,...(typeof item.reset==='string'&&item.reset.length<=100?{reset:item.reset}:{})}];
  });
  const windows=Array.isArray(data.windows)?data.windows.flatMap((window:unknown):NonNullable<AntigravityUsage['windows']>=>{
   if(!window||typeof window!=='object')return [];
   const item=window as {group?:unknown;window?:unknown;remaining?:unknown;reset?:unknown};
   if(typeof item.group!=='string'||!item.group.trim()||item.group.length>150||(item.window!=='5h'&&item.window!=='weekly')||typeof item.remaining!=='number'||!Number.isFinite(item.remaining)||item.remaining<0||item.remaining>1)return [];
   return [{group:item.group,window:item.window,remaining:item.remaining,...(typeof item.reset==='string'&&item.reset.length<=100?{reset:item.reset}:{})}];
  }):undefined;
  return [{email:data.email,...(typeof data.plan==='string'&&data.plan.length<=300?{plan:data.plan}:{}),models,...(windows?{windows}:{})}];
 });
}

/** Never assign a quota to an account without exactly one matching live identity. */
export function matchAntigravityUsage(email:string|null|undefined, samples:AntigravityUsage[]):AntigravityUsage|null {
 const identity=email?.trim().toLocaleLowerCase('en-US');
 if(!identity||!identity.includes('@'))return null;
 const matches=samples.filter(sample=>sample.email?.trim().toLocaleLowerCase('en-US')===identity);
 return matches.length===1?matches[0]:null;
}
