export function readAccountOrder():string[]{
  try{
    const value:unknown=JSON.parse(localStorage.getItem('nivra-account-order-v1')||'[]');
    return Array.isArray(value)?[...new Set(value.filter((id):id is string=>typeof id==='string'&&id.length>0&&id.length<=200))].slice(0,1000):[];
  }catch{return []}
}

/** Missing and removed accounts never prevent new accounts from being displayed. */
export function applyAccountOrder<T extends {id:string}>(accounts:T[],order:string[]):T[]{
  const ranks=new Map(order.map((id,index)=>[id,index]));
  return [...accounts].sort((a,b)=>(ranks.get(a.id)??order.length)-(ranks.get(b.id)??order.length));
}

/** Reorder the visible accounts while keeping other services in their existing slots. */
export function reorderVisibleEntries<T extends {id:string;service:string}>(
  entries:T[], service:string, sourceId:string, targetId:string, after:boolean,
):T[] {
  if(sourceId===targetId)return entries;
  const matches=(entry:T)=>service==='Tous'||entry.service===service;
  const visible=entries.filter(matches);
  const source=visible.findIndex(entry=>entry.id===sourceId);
  const target=visible.findIndex(entry=>entry.id===targetId);
  if(source<0||target<0)return entries;
  const [moved]=visible.splice(source,1);
  const insertAt=visible.findIndex(entry=>entry.id===targetId)+(after?1:0);
  if(insertAt===source)return entries;
  visible.splice(insertAt,0,moved);
  let index=0;
  return entries.map(entry=>matches(entry)?visible[index++]:entry);
}
