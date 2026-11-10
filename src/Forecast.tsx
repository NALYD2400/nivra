import { useEffect, useId, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent } from 'react';
import { createPortal } from 'react-dom';
import { ArrowUpRight, CalendarClock, Check, ChevronDown, Info, LineChart } from 'lucide-react';
import openaiWhite from './assets/brands/openai-white.svg';
import openaiBlack from './assets/brands/openai-black.svg';
import claudeIcon from './assets/brands/claude.svg';
import { estimateUsage, inspectUsage, usagePace, type UsagePoint, type UsageWindow } from './usage-forecast';

type Account = {id:string;name:string;service:'Codex'|'Claude Code'};
type Props = {accounts:Account[];history:UsagePoint[];icon:string;onOpenAccounts:()=>void};

function AccountLogo({service}:{service:Account['service']}){return <span className="forecast-provider-mark" aria-hidden="true">{service==='Claude Code'?<img src={claudeIcon} alt=""/>:<><img className="forecast-logo-dark" src={openaiWhite} alt=""/><img className="forecast-logo-light" src={openaiBlack} alt=""/></>}</span>}

function AccountSelect({accounts,current,onSelect}:{accounts:Account[];current:string;onSelect:(id:string)=>void}){
 const [open,setOpen]=useState(false);
 const container=useRef<HTMLDivElement>(null);
 const trigger=useRef<HTMLButtonElement>(null);
 const list=useRef<HTMLDivElement>(null);
 useEffect(()=>{
  if(!open)return;
  list.current?.querySelector<HTMLButtonElement>('[aria-selected="true"]')?.focus();
  const outside=(event:PointerEvent)=>{if(!container.current?.contains(event.target as Node))setOpen(false)};
  document.addEventListener('pointerdown',outside);
  return()=>document.removeEventListener('pointerdown',outside);
 },[open]);
 const close=()=>{setOpen(false);trigger.current?.focus()};
 const onListKey=(event:KeyboardEvent<HTMLDivElement>)=>{
  if(event.key==='Escape'){event.preventDefault();close();return}
  if(event.key==='Tab'){setOpen(false);return}
  if(!['ArrowDown','ArrowUp','Home','End'].includes(event.key))return;
  event.preventDefault();
  const options=Array.from(list.current?.querySelectorAll<HTMLButtonElement>('[role="option"]')??[]);
  const index=options.indexOf(document.activeElement as HTMLButtonElement);
  const next=event.key==='Home'?0:event.key==='End'?options.length-1:event.key==='ArrowDown'?(index+1)%options.length:(index-1+options.length)%options.length;
  options[next]?.focus();
 };
 return <div className="forecast-account-select" ref={container}>
  <button ref={trigger} type="button" className="forecast-select-trigger" aria-label="Choisir un compte" aria-haspopup="listbox" aria-expanded={open} aria-controls="forecast-account-options" onClick={()=>setOpen(!open)} onKeyDown={event=>{if(event.key==='ArrowDown'||event.key==='ArrowUp'){event.preventDefault();setOpen(true)}}}><AccountLogo service={accounts.find(account=>account.id===current)?.service??'Codex'}/><span key={current} className="forecast-selected-name">{accounts.find(account=>account.id===current)?.name}</span><ChevronDown size={15} className={open?'is-open':''}/></button>
  {open&&<div ref={list} id="forecast-account-options" className="forecast-select-menu" role="listbox" aria-label="Comptes suivis" onKeyDown={onListKey}>{accounts.map(account=><button key={account.id} type="button" role="option" aria-selected={account.id===current} className="forecast-select-option" onClick={()=>{onSelect(account.id);close()}}><AccountLogo service={account.service}/><span>{account.name}</span>{account.id===current&&<Check size={14} className="forecast-option-check"/>}</button>)}</div>}
 </div>;
}

function dateTime(time:number){return new Intl.DateTimeFormat('fr-FR',{day:'numeric',month:'short',hour:'2-digit',minute:'2-digit'}).format(time)}
function fullDateTime(time:number){return new Intl.DateTimeFormat('fr-FR',{weekday:'long',day:'numeric',month:'long',hour:'2-digit',minute:'2-digit'}).format(time)}
function duration(minutes:number|null,fallback:string){if(!minutes)return fallback;if(minutes===10080)return 'Semaine';if(minutes>=1440)return `${Math.round(minutes/1440*10)/10} j`;if(minutes>=60)return `${Math.round(minutes/60*10)/10} h`;return `${minutes} min`}

function usePlotScale(active:boolean){
 const ref=useRef<SVGSVGElement>(null);
 const [scale,setScale]=useState({x:1,y:1});
 useLayoutEffect(()=>{
  const node=ref.current;
  if(!node)return;
  const update=()=>{const bounds=node.getBoundingClientRect();if(bounds.width&&bounds.height)setScale({x:600/bounds.width,y:120/bounds.height})};
  update();
  const observer=new ResizeObserver(update);
  observer.observe(node);
  return()=>observer.disconnect();
 },[active]);
 return {ref,scale};
}

type ChartSeries={account:Account;color:string;points:UsagePoint[]};
function QuotaChart({series,now,label}:{series:ChartSeries[];now:number;label:string}){
 const available=series.filter(item=>item.points.length>0);
 const {ref:plot,scale}=usePlotScale(available.length>0);
 const tooltipId=useId();
 const tooltip=useRef<HTMLDivElement>(null);
 const pointerFrame=useRef<number|null>(null);
 const pendingPointer=useRef<{x:number;y:number}|null>(null);
 const [cursor,setCursor]=useState<{id:string;at:number;x:number;y:number}|null>(null);
 const [retainedCursor,setRetainedCursor]=useState<typeof cursor>(null);
 const shownCursor=cursor??retainedCursor;
 useEffect(()=>{if(cursor){setRetainedCursor(cursor);return}const timer=window.setTimeout(()=>setRetainedCursor(null),180);return()=>window.clearTimeout(timer)},[cursor]);
 useEffect(()=>()=>{if(pointerFrame.current!==null)cancelAnimationFrame(pointerFrame.current)},[]);
 const [position,setPosition]=useState({left:0,top:0});
 const charts=useMemo(()=>series.filter(item=>item.points.length>0).map(item=>{const last=item.points[item.points.length-1];return {...item,last,pace:usagePace(last,now),estimate:estimateUsage(item.points,now)}}),[series,now]);
 const start=charts.length?Math.min(...charts.map(item=>item.pace?.start??item.points[0].at)):now;
 const end=charts.length?Math.max(start+1,...charts.map(item=>item.last.resetAt*1000)):now+1;
 const x=(at:number)=>Math.max(0,Math.min(600,(at-start)/(end-start)*600));
 const y=(remaining:number)=>112-Math.max(0,Math.min(100,remaining))*1.04;
 const active=charts.find(item=>item.account.id===shownCursor?.id);
 const inspected=active&&shownCursor?inspectUsage(active.points,shownCursor.at,now):null;
 const targetAtCursor=active?.pace&&shownCursor&&shownCursor.at>=active.pace.start&&shownCursor.at<=active.pace.end?(active.pace.end-shownCursor.at)/(active.pace.end-active.pace.start)*100:null;
 useLayoutEffect(()=>{
  if(!shownCursor||!tooltip.current)return;
  const {width,height}=tooltip.current.getBoundingClientRect();
  const left=shownCursor.x+width+18<innerWidth?shownCursor.x+14:shownCursor.x-width-14;
  const top=shownCursor.y+height+18<innerHeight?shownCursor.y+14:shownCursor.y-height-14;
  setPosition({left:Math.max(8,Math.min(left,innerWidth-width-8)),top:Math.max(8,Math.min(top,innerHeight-height-8))});
 },[shownCursor]);
 function inspectAt(clientX:number,clientY:number){
  const bounds=plot.current?.getBoundingClientRect();if(!bounds||!charts.length)return;
  const ratio=Math.max(0,Math.min(1,(clientX-bounds.left)/bounds.width));
  const at=start+ratio*(end-start);
  const remaining=Math.max(0,Math.min(100,(112-(clientY-bounds.top)/bounds.height*120)/1.04));
  const sorted=charts.map(item=>{const data=inspectUsage(item.points,at,now);const guide=item.pace&&at>=item.pace.start&&at<=item.pace.end?(item.pace.end-at)/(item.pace.end-item.pace.start)*100:null;return {item,distance:Math.min(...[data?.predictedRemaining,data?.point?100-data.point.used:null,guide].filter((v):v is number=>v!==null&&v!==undefined).map(v=>Math.abs(v-remaining)))}}).sort((a,b)=>a.distance-b.distance);
  const previous=sorted.find(entry=>entry.item.account.id===cursor?.id);
  const nearest=previous&&previous.distance<=sorted[0].distance+2?previous:sorted[0];
  setCursor({id:nearest.item.account.id,at,x:clientX,y:clientY});
 }
 function stopInspect(){if(pointerFrame.current!==null){cancelAnimationFrame(pointerFrame.current);pointerFrame.current=null}pendingPointer.current=null;setCursor(null)}
 function queueInspect(clientX:number,clientY:number){
  pendingPointer.current={x:clientX,y:clientY};
  if(pointerFrame.current!==null)return;
  pointerFrame.current=requestAnimationFrame(()=>{pointerFrame.current=null;const point=pendingPointer.current;if(point)inspectAt(point.x,point.y)});
 }
 function keyboard(event:KeyboardEvent<SVGSVGElement>){
  if(event.key==='Escape'){stopInspect();return}
  if(!['ArrowLeft','ArrowRight','ArrowUp','ArrowDown','Home','End'].includes(event.key)||!charts.length)return;
  event.preventDefault();
  let index=Math.max(0,charts.findIndex(item=>item.account.id===cursor?.id));
  if(event.key==='ArrowUp')index=(index-1+charts.length)%charts.length;
  if(event.key==='ArrowDown')index=(index+1)%charts.length;
  const item=charts[index];const step=(end-start)/50;
  const at=Math.max(start,Math.min(end,event.key==='Home'?start:event.key==='End'?end:(cursor?.at??item.last.at)+(event.key==='ArrowLeft'?-step:event.key==='ArrowRight'?step:0)));
  const bounds=plot.current!.getBoundingClientRect();
  setCursor({id:item.account.id,at,x:bounds.left+x(at)/600*bounds.width,y:bounds.top+bounds.height/2});
 }
 if(!charts.length)return <div className="forecast-chart-empty"><LineChart size={21}/><span>Aucun relevé disponible pour cette limite.</span></div>;
 const anyProjection=charts.some(item=>item.estimate.rate!==undefined);
 return <div className="forecast-chart interactive-quota-chart">
  <div className="forecast-chart-heading"><span>{charts.length>1?`${charts.length} comptes`:'Quota restant'}</span><div className="forecast-legend"><span><i/> Relevés</span>{charts.some(item=>item.pace)&&<span><i className="forecast-legend-pace"/> Rythme conseillé</span>}{anyProjection&&<span><i className="forecast-legend-projection"/> Prévision</span>}</div></div>
  <div className="forecast-plot"><div className="forecast-plot-labels" aria-hidden="true"><span>100 %</span><span>50 %</span><span>0 %</span></div>
   <svg ref={plot} viewBox="0 0 600 120" preserveAspectRatio="none" tabIndex={0} role="group" aria-label={`${label}. Survole le graphique pour les détails. Au clavier, gauche et droite parcourent le temps, haut et bas changent de compte, Échap ferme les détails.`} aria-describedby={cursor?tooltipId:undefined} onPointerMove={event=>queueInspect(event.clientX,event.clientY)} onPointerDown={event=>inspectAt(event.clientX,event.clientY)} onPointerLeave={stopInspect} onBlur={stopInspect} onFocus={()=>{const item=charts[0];const bounds=plot.current!.getBoundingClientRect();setCursor({id:item.account.id,at:item.last.at,x:bounds.left+x(item.last.at)/600*bounds.width,y:bounds.top+bounds.height/2})}} onKeyDown={keyboard}>
    {[0,25,50,75,100].map(value=><line key={value} x1="0" x2="600" y1={y(value)} y2={y(value)} className="forecast-grid-line"/>)}
    {charts.map(({account,color,points,last,pace,estimate})=>{const projection=estimate.rate!==undefined;const target=estimate.kind==='projected'&&estimate.at?estimate.at:last.resetAt*1000;return <g className="quota-series" key={account.id} opacity={cursor&&cursor.id!==account.id?0.25:1}>
     {pace&&<path d={`M${x(pace.start)},${y(100)} L${x(pace.end)},${y(0)}`} className="forecast-pace-line" style={charts.length>1?{stroke:color}:undefined} vectorEffect="non-scaling-stroke"/>}
     {points.length>1&&<path d={points.map((point,i)=>`${i?'L':'M'}${x(point.at)},${y(100-point.used)}`).join(' ')} fill="none" stroke={color} strokeWidth="2.5" strokeLinejoin="round" vectorEffect="non-scaling-stroke"/>}
     <ellipse cx={x(last.at)} cy={y(100-last.used)} rx={3.5*scale.x} ry={3.5*scale.y} fill={last.used>=100?'var(--quota-depleted)':color}/>
     {projection&&<path d={`M${x(last.at)},${y(100-last.used)} L${x(target)},${y(Math.max(0,100-last.used-(target-last.at)*estimate.rate!))}`} className="forecast-predicted-line" style={{stroke:color}} vectorEffect="non-scaling-stroke"/>}
    </g>})}
    {cursor&&active&&inspected&&<g pointerEvents="none" className="quota-inspection-points"><g className="quota-tracker" style={{transform:`translateX(${x(cursor.at)}px)`}}><line x1="0" x2="0" y1="8" y2="112" className="forecast-cursor-line" vectorEffect="non-scaling-stroke"/>{targetAtCursor!==null&&<ellipse cx="0" cy={y(targetAtCursor)} rx={4*scale.x} ry={4*scale.y} className="forecast-pace-dot"/>}{inspected.predictedRemaining!==null&&<ellipse cx="0" cy={y(inspected.predictedRemaining)} rx={4*scale.x} ry={4*scale.y} fill={active.color}/>}</g><ellipse cx={x(inspected.point.at)} cy={y(100-inspected.point.used)} rx={5*scale.x} ry={5*scale.y} fill={active.color}/></g>}
   </svg>
  </div><div className="forecast-chart-dates"><span>{dateTime(start)}</span><span>{dateTime(end)}</span></div>
  <div className="forecast-interaction-hint">Survole pour explorer{charts.length>1?' · ↑ ↓ pour changer de compte':''}</div>
  {shownCursor&&active&&inspected&&createPortal(<div ref={tooltip} id={tooltipId} role="tooltip" aria-hidden={!cursor} className={`quota-inspector${cursor?" is-visible":""}`} style={{transform:`translate3d(${position.left}px,${position.top}px,0)`}}><div className="quota-inspector-surface">
   <div className="quota-inspector-title"><span className="comparison-dot" style={{backgroundColor:active.color}}/><strong>{active.account.name}</strong></div>
   <p className="quota-inspector-date">{dateTime(shownCursor.at)}</p>
   <dl><div><dt>Relevé · {dateTime(inspected.point.at)}</dt><dd>{Math.round(100-inspected.point.used)} % restants</dd></div>
   {targetAtCursor!==null&&<div><dt>Rythme conseillé à cette date</dt><dd>{Math.round(targetAtCursor)} % restants</dd></div>}
   {inspected.predictedRemaining!==null&&<div><dt>Prévision à cette date</dt><dd>{Math.round(inspected.predictedRemaining)} % restants</dd></div>}
   {active.pace&&<div><dt>Budget actuel</dt><dd>{new Intl.NumberFormat('fr-FR',{maximumFractionDigits:1}).format(active.pace.hourlyBudget*(active.last.durationMins!>=1440?24:1))} % /{active.last.durationMins!>=1440?'jour':'heure'}</dd></div>}
   <div><dt>Renouvellement</dt><dd>{dateTime(active.last.resetAt*1000)}</dd></div></dl>
   <p className="quota-inspector-note">{active.estimate.kind==='stale'?'Relevés à actualiser.':active.last.used>=100?'Quota épuisé : attends le renouvellement.':shownCursor.at<active.points[0].at?'Aucun relevé avant le début du suivi.':active.pace&&active.pace.gap < -1?`${Math.round(-active.pace.gap)} points consommés en avance sur le rythme régulier.`:'Les petits pointillés indiquent le rythme à suivre.'}</p>
   {active.estimate.kind==='projected'&&active.estimate.at&&<p className="quota-inspector-note">Épuisement estimé : {dateTime(active.estimate.at)}</p>}
  </div></div>,document.body)}
 </div>;
}

function WindowForecast({accountId,accountName,window,history,now}:{accountId:string;accountName:string;window:UsageWindow;history:UsagePoint[];now:number}){
 const all=history.filter(p=>p.accountId===accountId&&p.window===window).sort((a,b)=>a.at-b.at);
 const last=all[all.length-1];
 const points=last?all.filter(p=>p.resetAt===last.resetAt):[];
 const estimate=estimateUsage(points,now);
 const name=duration(last?.durationMins??null,window==='primary'?'Limite principale':'Limite secondaire');
 const remaining=last?Math.max(0,Math.min(100,Math.round(100-last.used))):null;
 let headline='En attente de relevés';
 if(estimate.kind==='insufficient'&&last){headline='Pas encore assez de recul'}
 if(estimate.kind==='stale'){headline='Données à actualiser'}
 if(estimate.kind==='unchanged'){headline='Consommation stable'}
 if(estimate.kind==='after-reset'){headline='Devrait tenir jusqu’au renouvellement'}
 if(estimate.kind==='depleted'){headline='Limite atteinte'}
 if(estimate.kind==='projected'&&estimate.at){headline=estimate.at<=now?'Ton quota est peut-être épuisé':'Épuisement prévu avant renouvellement'}
 return <section className="forecast-card"><div className="forecast-card-head"><div><h2>{name}</h2><p className="forecast-status">{headline}</p></div><div className="forecast-remaining">{remaining===null?'—':<>{remaining}<small>%</small></>}<span>restant</span></div></div>
  <div className="forecast-reset"><CalendarClock size={14}/><span>Renouvellement :</span><strong>{last?fullDateTime(last.resetAt*1000):'date indisponible'}</strong></div>
  <QuotaChart series={[{account:{id:accountId,name:accountName,service:'Codex'},color:'#c3a8ff',points}]} now={now} label={name}/>
 </section>;
}

const accountColors=['#c3a8ff','#70d8c2','#ffb68a','#83bbff','#f497cb','#f3d379','#b9dd8f','#d6a3ef'];
function ComparisonWindow({accounts,history,window,now}:{accounts:Account[];history:UsagePoint[];window:UsageWindow;now:number}){
 const series=accounts.map((account,index)=>{const all=history.filter(point=>point.accountId===account.id&&point.window===window).sort((a,b)=>a.at-b.at);const last=all[all.length-1];return {account,color:accountColors[index%accountColors.length],points:last?all.filter(point=>point.resetAt===last.resetAt):[]}});
 const durations=[...new Set(series.map(item=>item.points.at(-1)?.durationMins).filter((value):value is number=>!!value))];
 const name=duration(durations.length===1?durations[0]:null,window==='primary'?'Limite principale':'Limite secondaire');
 const missing=series.filter(item=>!item.points.length).length;
 return <section className="forecast-card comparison-card"><div className="comparison-head"><h2>{name}</h2><span>Quota restant</span></div><QuotaChart series={series} now={now} label={name}/>{missing>0&&<p className="comparison-unavailable">{missing} compte{missing>1?'s':''} sans quota disponible</p>}</section>;
}

export function Forecast({accounts,history,icon,onOpenAccounts}:Props){
 const [selected,setSelected]=useState('');
 const [mode,setMode]=useState<'one'|'all'>('one');
 function switchMode(next:'one'|'all'){if(next!==mode)setMode(next)}
 const [now,setNow]=useState(()=>Date.now());
 useEffect(()=>{const timer=window.setInterval(()=>setNow(Date.now()),60000);return()=>window.clearInterval(timer)},[]);
 const current=accounts.some(a=>a.id===selected)?selected:accounts[0]?.id;
 const view=useRef<HTMLDivElement>(null);
 const viewKey=`${mode}:${current??''}`;
 const previousView=useRef(viewKey);
 useLayoutEffect(()=>{
  if(previousView.current===viewKey)return;
  previousView.current=viewKey;
  if(matchMedia('(prefers-reduced-motion: reduce)').matches)return;
  const animations=Array.from(view.current?.querySelectorAll<HTMLElement>('.forecast-card,.forecast-no-account')??[]).map((node,index)=>node.animate?.([{opacity:.45,transform:'translateY(9px)'},{opacity:1,transform:'translateY(0)'}],{duration:620,delay:index*55,easing:'cubic-bezier(.16,1,.3,1)',fill:'backwards'}));
  return()=>animations.forEach(animation=>animation?.cancel());
 },[viewKey]);
 return <div className="forecast-page"><header className="forecast-header"><div className="forecast-heading"><img src={icon} alt=""/><h1>Prévisions</h1></div></header>
 {accounts.length===0?<div className="empty forecast-no-account"><div className="forecast-empty-icon"><LineChart size={23}/></div><div><b>Aucun compte suivi</b><p>Connecte un compte pour commencer à recueillir ses relevés.</p><button className="text-button" onClick={onOpenAccounts}>Voir mes comptes <ArrowUpRight size={14}/></button></div></div>:<><div className="forecast-controls"><div className="forecast-control-actions"><div className="forecast-mode" data-mode={mode} role="group" aria-label="Affichage des prévisions"><button type="button" title="Voir le quota restant et la prévision du compte sélectionné." aria-pressed={mode==='one'} onClick={()=>switchMode('one')}>Un compte</button><button type="button" title="Comparer les quotas restants et leur évolution pour tous tes comptes." aria-pressed={mode==='all'} onClick={()=>switchMode('all')}>Tous les comptes</button></div>{mode==='one'&&<AccountSelect accounts={accounts} current={current!} onSelect={setSelected}/>}</div></div>
 <div ref={view} key={viewKey} className="forecast-view">{mode==='all'?<div className="forecast-list"><ComparisonWindow accounts={accounts} window="primary" history={history} now={now}/><ComparisonWindow accounts={accounts} window="secondary" history={history} now={now}/></div>:accounts.find(account=>account.id===current)?.service==='Claude Code'?<div className="empty forecast-no-account"><Info size={20}/><div><b>Quotas Claude indisponibles</b><p>Nivra ne peut pas encore lire les quotas de ce compte.</p></div></div>:<div className="forecast-list"><WindowForecast accountName={accounts.find(account=>account.id===current)!.name} accountId={current!} window="primary" history={history} now={now}/><WindowForecast accountName={accounts.find(account=>account.id===current)!.name} accountId={current!} window="secondary" history={history} now={now}/></div>}</div>
 </>}
 </div>;
}

