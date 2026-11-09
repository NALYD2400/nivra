import { useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent } from 'react';
import { createPortal } from 'react-dom';
import { ArrowUpRight, CalendarClock, Check, ChevronDown, Info, LineChart } from 'lucide-react';
import openaiWhite from './assets/brands/openai-white.svg';
import openaiBlack from './assets/brands/openai-black.svg';
import claudeIcon from './assets/brands/claude.svg';
import { estimateUsage, type UsagePoint, type UsageWindow } from './usage-forecast';

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
  <button ref={trigger} type="button" className="forecast-select-trigger" aria-label="Choisir un compte" aria-haspopup="listbox" aria-expanded={open} aria-controls="forecast-account-options" onClick={()=>setOpen(!open)} onKeyDown={event=>{if(event.key==='ArrowDown'||event.key==='ArrowUp'){event.preventDefault();setOpen(true)}}}><AccountLogo service={accounts.find(account=>account.id===current)?.service??'Codex'}/><span>{accounts.find(account=>account.id===current)?.name}</span><ChevronDown size={15} className={open?'is-open':''}/></button>
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

function TrendChart({points,now,label}:{points:UsagePoint[];now:number;label:string}){
 const {ref:plot,scale}=usePlotScale(points.length>0);
 const latest=points[points.length-1];
 if(!latest)return <div className="forecast-chart-empty"><LineChart size={21}/><span>Le graphique apparaîtra avec les premiers relevés.</span></div>;
 const estimate=estimateUsage(points,now);
 const left=0,right=600,top=8,bottom=112;
 const start=points[0].at,end=Math.max(latest.resetAt*1000,start+1);
 const x=(at:number)=>left+Math.max(0,Math.min(1,(at-start)/(end-start)))*(right-left);
 const y=(remaining:number)=>bottom-Math.max(0,Math.min(100,remaining))/100*(bottom-top);
 const line=points.map((point,index)=>`${index?'L':'M'}${x(point.at).toFixed(1)},${y(100-point.used).toFixed(1)}`).join(' ');
 const canProject=(estimate.kind==='projected'||estimate.kind==='after-reset')&&estimate.rate!==undefined;
 const target=estimate.kind==='projected'&&estimate.at?estimate.at:end;
 const endRemaining=canProject?Math.max(0,100-latest.used-(target-latest.at)*estimate.rate!):0;
 return <div className="forecast-chart"><div className="forecast-chart-heading"><span>Évolution du quota</span>{canProject&&<div className="forecast-legend"><span><i/> Observé</span><span><i className="forecast-legend-projection"/> Estimation</span></div>}</div><div className="forecast-plot"><div className="forecast-plot-labels" aria-hidden="true"><span>100 %</span><span>50 %</span><span>0 %</span></div><svg ref={plot} viewBox="0 0 600 120" preserveAspectRatio="none" role="img" aria-label={`Évolution du quota restant : ${label}. La ligne pointillée est une estimation.`}>
  {[0,50,100].map(value=><line key={value} x1={left} x2={right} y1={y(value)} y2={y(value)} className="forecast-grid-line"/>)}
  {points.length>1&&<path d={line} className="forecast-history-line" vectorEffect="non-scaling-stroke"/>}
  <ellipse cx={x(latest.at)} cy={y(100-latest.used)} rx={3.5*scale.x} ry={3.5*scale.y} className="forecast-current-dot"/>
  {canProject&&<path d={`M${x(latest.at)},${y(100-latest.used)} L${x(target)},${y(endRemaining)}`} className="forecast-predicted-line" vectorEffect="non-scaling-stroke"/>}
  {points.map((point,index)=><ellipse key={`${point.at}-${index}`} cx={x(point.at)} cy={y(100-point.used)} rx={9*scale.x} ry={9*scale.y} fill="transparent" className="forecast-point-target"><title>{`${dateTime(point.at)} · ${Math.round(Math.max(0,Math.min(100,100-point.used)))} % de quota restant${index===points.length-1?' · Dernier relevé':''}`}</title></ellipse>)}
 </svg></div><div className="forecast-chart-dates"><span>{dateTime(start)}</span><span>{dateTime(end)}</span></div></div>;
}

function WindowForecast({accountId,window,history,now}:{accountId:string;window:UsageWindow;history:UsagePoint[];now:number}){
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
 if(estimate.kind==='projected'&&estimate.at){headline=estimate.at<=now?'Ton quota est peut-être épuisé':`Quota épuisé estimé : ${fullDateTime(estimate.at)}`}
 return <section className="forecast-card"><div className="forecast-card-head"><div><h2>{name}</h2><p className="forecast-status">{headline}</p></div><div className="forecast-remaining">{remaining===null?'—':<>{remaining}<small>%</small></>}<span>restant</span></div></div>
  <div className="forecast-reset"><CalendarClock size={14}/><span>Renouvellement :</span><strong>{last?fullDateTime(last.resetAt*1000):'date indisponible'}</strong></div>
  <TrendChart points={points} now={now} label={name}/>
 </section>;
}

const accountColors=['#c3a8ff','#70d8c2','#ffb68a','#83bbff','#f497cb','#f3d379','#b9dd8f','#d6a3ef'];
function ComparisonWindow({accounts,history,window,now}:{accounts:Account[];history:UsagePoint[];window:UsageWindow;now:number}){
 const [hovered,setHovered]=useState<string|null>(null);
 const [nearby,setNearby]=useState<string[]>([]);
 const {ref:plot,scale}=usePlotScale(history.some(point=>point.window===window&&accounts.some(account=>account.id===point.accountId)));
 const tooltip=useRef<HTMLDivElement>(null);
 const [pointer,setPointer]=useState({x:0,y:0});
 const [tipPosition,setTipPosition]=useState({left:0,top:0});
 useLayoutEffect(()=>{
  if(!hovered||!tooltip.current)return;
  const {width,height}=tooltip.current.getBoundingClientRect();
  const gap=14, margin=8;
  const left=pointer.x+gap+width<=globalThis.innerWidth-margin?pointer.x+gap:pointer.x-gap-width;
  const top=pointer.y+gap+height<=globalThis.innerHeight-margin?pointer.y+gap:pointer.y-gap-height;
  setTipPosition({left:Math.max(margin,Math.min(left,globalThis.innerWidth-width-margin)),top:Math.max(margin,Math.min(top,globalThis.innerHeight-height-margin))});
 },[hovered,nearby,pointer]);
 const series=accounts.map((account,index)=>{
  const all=history.filter(p=>p.accountId===account.id&&p.window===window).sort((a,b)=>a.at-b.at);
  const last=all[all.length-1];
  const points=last?all.filter(p=>p.resetAt===last.resetAt):[];
  return {account,color:accountColors[index%accountColors.length],last,points,estimate:estimateUsage(points,now)};
 });
 const plotted=series.filter(s=>s.last);
 const start=plotted.length?Math.min(...plotted.map(s=>s.points[0].at)):now;
 const end=plotted.length?Math.max(start+1,...plotted.map(s=>s.last!.resetAt*1000)):now+1;
 const x=(at:number)=>Math.max(0,Math.min(600,(at-start)/(end-start)*600));
 const y=(used:number)=>8+Math.max(0,Math.min(100,used))*1.04;
 const durations=[...new Set(plotted.map(s=>s.last?.durationMins).filter((value):value is number=>!!value))];
 const name=duration(durations.length===1?durations[0]:null,window==='primary'?'Limite principale':'Limite secondaire');
 const showAccount=(id:string)=>{
  const active=series.find(item=>item.account.id===id);
  if(!active?.last)return;
  const bounds=plot.current?.getBoundingClientRect();
  const width=bounds?.width||600, height=bounds?.height||120;
  const last=active.last;
  setNearby(plotted.filter(item=>Math.hypot((x(item.last!.at)-x(last.at))*width/600,(y(item.last!.used)-y(last.used))*height/120)<=12).map(item=>item.account.id));
  setHovered(id);
 };
 return <section className="forecast-card comparison-card" onMouseLeave={()=>setHovered(null)}><div className="comparison-head"><h2>{name}</h2><span>Quota restant</span></div>
 {plotted.length?<><div className="forecast-chart"><div className="forecast-plot"><div className="forecast-plot-labels" aria-hidden="true"><span>100 %</span><span>50 %</span><span>0 %</span></div><svg ref={plot} viewBox="0 0 600 120" preserveAspectRatio="none" role="group" aria-label={`Comparaison du quota restant : ${window==='primary'?'limite principale':'limite secondaire'}. Les lignes pointillées sont des estimations.`}>
  {[0,50,100].map(v=><line key={v} x1="0" x2="600" y1={y(v)} y2={y(v)} className="forecast-grid-line"/>)}
  {plotted.map(({account,color,last,points,estimate})=>{const latest=last!;const line=points.map((p,i)=>`${i?'L':'M'}${x(p.at)},${y(p.used)}`).join(' ');const projected=(estimate.kind==='projected'||estimate.kind==='after-reset')&&estimate.rate!==undefined;const target=estimate.kind==='projected'&&estimate.at?estimate.at:latest.resetAt*1000;const targetUsed=projected?Math.min(100,latest.used+(target-latest.at)*estimate.rate!):latest.used;return <g key={account.id} tabIndex={0} role="button" aria-label={`${account.name} : ${Math.round(100-latest.used)} % restant. Renouvellement : ${fullDateTime(latest.resetAt*1000)}`} aria-describedby={hovered===account.id ? `comparison-tip-${window}` : undefined} onMouseEnter={event=>{setPointer({x:event.clientX,y:event.clientY});showAccount(account.id)}} onMouseMove={event=>setPointer({x:event.clientX,y:event.clientY})} onMouseLeave={()=>setHovered(null)} onFocus={event=>{const rect=event.currentTarget.getBoundingClientRect();setPointer({x:rect.left+rect.width/2,y:rect.top+rect.height/2});showAccount(account.id)}} onBlur={()=>setHovered(null)} onKeyDown={event=>{if(event.key==='Escape')setHovered(null)}}><path d={line} fill="none" stroke={color} strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round" vectorEffect="non-scaling-stroke"/><ellipse cx={x(latest.at)} cy={y(latest.used)} rx={3.5*scale.x} ry={3.5*scale.y} fill={color}/><ellipse cx={x(latest.at)} cy={y(latest.used)} rx={10*scale.x} ry={10*scale.y} fill="transparent"/>{projected&&<path d={`M${x(latest.at)},${y(latest.used)} L${x(target)},${y(targetUsed)}`} fill="none" stroke={color} strokeWidth="2" strokeDasharray="5 5" opacity=".7" vectorEffect="non-scaling-stroke"/>}</g>})}
 </svg></div><div className="forecast-chart-dates"><span>{dateTime(start)}</span><span>{dateTime(end)}</span></div></div></>:<div className="forecast-chart-empty"><LineChart size={20}/><span>Aucun relevé disponible pour cette limite.</span></div>}
 {hovered&&createPortal(<div ref={tooltip} className="comparison-tooltip" id={`comparison-tip-${window}`} role="tooltip" style={{left:tipPosition.left,top:tipPosition.top}}><div className="comparison-tooltip-heading">{nearby.length>1?`${nearby.length} comptes à ce point`:'Quota restant'}</div>{series.filter(item=>nearby.includes(item.account.id)).map(({account,color,last,estimate})=><div className="comparison-tooltip-row" key={account.id}><span className="comparison-dot" style={{backgroundColor:color}}/><AccountLogo service={account.service}/><strong title={account.name}>{account.name}</strong>{estimate.kind==='stale'&&<span className="comparison-tooltip-stale">À actualiser</span>}<b>{Math.round(Math.max(0,Math.min(100,100-last!.used)))} %</b></div>)}</div>,document.body)}
 {series.some(item=>!item.last)&&<p className="comparison-unavailable" title={series.filter(item=>!item.last).map(item=>item.account.name).join(', ')}>{series.filter(item=>!item.last).length} compte{series.filter(item=>!item.last).length>1?'s':' '} sans quota disponible</p>}</section>;
}

export function Forecast({accounts,history,icon,onOpenAccounts}:Props){
 const [selected,setSelected]=useState('');
 const [mode,setMode]=useState<'one'|'all'>('one');
 const [hasSwitched,setHasSwitched]=useState(false);
 function switchMode(next:'one'|'all'){if(next===mode)return;setHasSwitched(true);setMode(next)}
 const [now,setNow]=useState(()=>Date.now());
 useEffect(()=>{const timer=window.setInterval(()=>setNow(Date.now()),60000);return()=>window.clearInterval(timer)},[]);
 const current=accounts.some(a=>a.id===selected)?selected:accounts[0]?.id;
 return <div key={mode} className={hasSwitched ? "forecast-page page-enter" : "forecast-page"}><header className="forecast-header"><div className="forecast-heading"><img src={icon} alt=""/><h1>Prévisions</h1></div></header>
 {accounts.length===0?<div className="empty forecast-no-account"><div className="forecast-empty-icon"><LineChart size={23}/></div><div><b>Aucun compte suivi</b><p>Connecte un compte pour commencer à recueillir ses relevés.</p><button className="text-button" onClick={onOpenAccounts}>Voir mes comptes <ArrowUpRight size={14}/></button></div></div>:<><div className="forecast-controls"><div className="forecast-control-actions"><div className="forecast-mode" data-mode={mode} role="group" aria-label="Affichage des prévisions"><button type="button" title="Voir le quota restant et la prévision du compte sélectionné." aria-pressed={mode==='one'} onClick={()=>switchMode('one')}>Un compte</button><button type="button" title="Comparer les quotas restants et leur évolution pour tous tes comptes." aria-pressed={mode==='all'} onClick={()=>switchMode('all')}>Tous les comptes</button></div>{mode==='one'&&<AccountSelect accounts={accounts} current={current!} onSelect={setSelected}/>}</div></div>
 <div className="forecast-view">{mode==='all'?<div className="forecast-list"><ComparisonWindow accounts={accounts} window="primary" history={history} now={now}/><ComparisonWindow accounts={accounts} window="secondary" history={history} now={now}/></div>:accounts.find(account=>account.id===current)?.service==='Claude Code'?<div className="empty forecast-no-account"><Info size={20}/><div><b>Quotas Claude indisponibles</b><p>Nivra ne peut pas encore lire les quotas de ce compte.</p></div></div>:<div className="forecast-list"><WindowForecast accountId={current!} window="primary" history={history} now={now}/><WindowForecast accountId={current!} window="secondary" history={history} now={now}/></div>}</div>
 </>}
 </div>;
}

