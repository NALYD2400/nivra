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
