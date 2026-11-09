import { useEffect, useMemo, useRef, useState, type FormEvent, type PointerEvent as ReactPointerEvent } from 'react';
import { createRoot } from 'react-dom/client';
import { flushSync } from 'react-dom';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { ArrowDownLeft, ArrowUpRight, ChevronDown, Clock3, ExternalLink, Info, Link2, LoaderCircle, Maximize2, Minus, Moon, PanelLeftClose, PanelLeftOpen, Plus, RefreshCw, Settings2, Sparkles, Sun, Trash2, X } from 'lucide-react';
import brandIcon from './assets/nivra-taskbar.png';
import summaryAccounts from './assets/summary-accounts.png';
import summaryServices from './assets/summary-services.png';
import summaryClock from './assets/summary-clock.png';
import reorderIcon from './assets/reorder-icon.png';
import overviewV2 from './assets/overview-v4.png';
import settingsIcon from './assets/settings-icon.png';
import menuIcon from './assets/menu-icon.png';
import openaiWhite from './assets/brands/openai-white.svg';
import openaiBlack from './assets/brands/openai-black.svg';
import claudeIcon from './assets/brands/claude.svg';
import cursorLight from './assets/brands/cursor-light.svg';
import cursorDark from './assets/brands/cursor-dark.svg';
import antigravityIcon from './assets/brands/antigravity.svg';
import { AppearanceSettings, readAppearance, type Appearance } from './Appearance';
import { Background } from './components/Background';
import { useNivraUpdater } from './Updater';
import { reorderVisibleEntries } from './reorder';
import RibbonGlow from './components/originkit/ui/ribbon-glow';
import './originkit.css';
import './style.css';

type Service = 'Codex' | 'Claude Code' | 'Cursor' | 'Antigravity';
type Entry = { id:string; service:Service; name:string; remaining:number|null; reset:string; updated:string; mode?:'codex'|'claude'|'local'|'cursor' };
type WindowLimit = { usedPercent:number; windowDurationMins:number|null; resetsAt:number|null };
type Result = { account:{email?:string; type?:string; planType?:string}|null; limits:{rateLimits?:{primary?:WindowLimit|null; secondary?:WindowLimit|null}} };
type AntigravityUsage = {email?:string;plan?:string;models:{label:string;remaining:number;reset?:string}[]};
type ClaudeAccount = {email?:string|null; subscriptionType?:string|null};
type Snapshot = { data?:Result; error?:string; updated?:string; loading?:boolean };
const services:Service[] = ['Codex','Claude Code','Cursor','Antigravity'];
const links:Record<Service,string> = { Codex:'https://chatgpt.com/settings/usage', 'Claude Code':'https://claude.ai/settings/usage', Cursor:'https://cursor.com/settings', Antigravity:'https://antigravity.google/' };
const descriptions:Record<Service,string> = { Codex:'OpenAI', 'Claude Code':'Anthropic', Cursor:'Cursor', Antigravity:'Google' };
function dateText(value:string|number|null|undefined) { if (!value) return 'Non renseignée'; const d = new Date(typeof value === 'number' ? value*1000 : value); return isNaN(d.getTime()) ? 'Non renseignée' : new Intl.DateTimeFormat('fr-FR',{dateStyle:'medium',timeStyle:'short'}).format(d); }
function shortTime(value?:string) { return value ? new Date(value).toLocaleTimeString('fr-FR',{hour:'2-digit',minute:'2-digit'}) : '—'; }
function period(minutes:number|null|undefined, fallback:string) { if (!minutes) return fallback; if (minutes >= 1440) return `${Math.round(minutes/1440*10)/10} jours`; if (minutes >= 60) return `${Math.round(minutes/60*10)/10} heures`; return `${minutes} min`; }
function amount(limit?:WindowLimit|null) { return limit && typeof limit.usedPercent === 'number' ? Math.max(0,Math.min(100,Math.round(100-limit.usedPercent))) : null; }
function BrandMark({service,small=false}:{service:Service;small?:boolean}) {
  const icon=service==='Claude Code'?<img src={claudeIcon} alt=""/>:service==='Antigravity'?<img src={antigravityIcon} alt=""/>:service==='Codex'?<><img className="brand-on-dark" src={openaiWhite} alt=""/><img className="brand-on-light" src={openaiBlack} alt=""/></>:<><img className="brand-on-dark" src={cursorDark} alt=""/><img className="brand-on-light" src={cursorLight} alt=""/></>;
  return <span className={`provider-mark ${service==='Claude Code'?'claude':service.toLowerCase()} ${small?'small':''}`} aria-hidden="true">{icon}</span>;
}
function Limit({value,label}:{value?:WindowLimit|null;label:string}) { const left=amount(value); return <div className="quota-window"><div className="quota-label"><span>{period(value?.windowDurationMins,label)}</span><span className="quota-number">{left === null ? '—' : <>{left}<span>%</span></>}</span></div><div className="progress" role="progressbar" aria-label={`Quota restant, ${period(value?.windowDurationMins,label)}`} aria-valuemin={0} aria-valuemax={100} aria-valuenow={left??undefined}><span style={{width:`${left??0}%`}} /></div><div className="quota-reset"><Clock3 size={12}/><span>{value?.resetsAt?`Se renouvelle le ${dateText(value.resetsAt)}`:'Réinitialisation indisponible'}</span></div></div>; }

function App() {
 const updater=useNivraUpdater();
 const [entries,setEntries]=useState<Entry[]>(()=>{try{const parsed=JSON.parse(localStorage.getItem('quota-entries')||'[]');return Array.isArray(parsed)?parsed:[]}catch{return []}});
 const [active,setActive]=useState<Snapshot>({});
 const [antigravityEmail,setAntigravityEmail]=useState<string|null>(null);
 const [antigravityLoading,setAntigravityLoading]=useState(false);
 const [antigravityUsage,setAntigravityUsage]=useState<AntigravityUsage|null>(null);
 const [profiles,setProfiles]=useState<Record<string,Snapshot>>({});
 const [cursorErrors,setCursorErrors]=useState<Record<string,string>>({});
 const [claudeProfiles,setClaudeProfiles]=useState<Record<string,{data?:ClaudeAccount; error?:string; loading?:boolean}>>({});
 const [page,setPage]=useState<Service|'Tous'|'Reglages'>('Tous');
 const draggingAccount=useRef<{id:string;pointerId:number;x:number;y:number;offset:number;moved:boolean}|null>(null);
 const dragCleanup=useRef<(()=>void)|null>(null);
 const cardAnimations=useRef(new WeakMap<HTMLElement,Animation>());
 const entriesRef=useRef(entries);entriesRef.current=entries;
 const [draggingId,setDraggingId]=useState<string|null>(null);
 useEffect(()=>()=>{dragCleanup.current?.()},[]);
 const [reorderMessage,setReorderMessage]=useState('');
 const hasNavigated=useRef(false);
 const mainRef=useRef<HTMLElement>(null);
 function navigate(next:Service|'Tous'|'Reglages'){if(next===page)return;hasNavigated.current=true;mainRef.current?.scrollTo({top:0,behavior:'instant'});setPage(next)}
 const [menuCollapsed,setMenuCollapsed]=useState(()=>localStorage.getItem('nivra-menu-collapsed')==='true');
 useEffect(()=>{localStorage.setItem('nivra-menu-collapsed',String(menuCollapsed))},[menuCollapsed]);
 const [appearance,setAppearance]=useState<Appearance>(readAppearance);
 useEffect(()=>{setAppearance(old=>old.ribbonRevision===2?old:{...old,accent:'violet',glow:true,glowIntensity:100,ribbonRevision:2})},[]);
 useEffect(()=>{const root=document.documentElement;root.dataset.theme=appearance.theme;root.dataset.accent=appearance.accent;root.dataset.density=appearance.density;localStorage.setItem('nivra-appearance',JSON.stringify(appearance))},[appearance]);
 const loginAttempt=useRef(0); const loginProfile=useRef<string|null>(null);
 function closeDialog(){loginAttempt.current++;const profile=loginProfile.current;loginProfile.current=null;if(profile&&isTauri())void invoke('cancel_login',{profile});setLoginBusy(false);setShow(false)}
 const [show,setShow]=useState(false); const [service,setService]=useState<Service>('Codex');
 const [loginBusy,setLoginBusy]=useState(false); const [loginError,setLoginError]=useState('');
 useEffect(()=>{localStorage.setItem('quota-entries',JSON.stringify(entries))},[entries]);
 async function refreshActive(){setActive(old=>({...old,loading:true,error:undefined}));try{if(!isTauri())throw new Error('Ouvre l’application Windows pour consulter les quotas.');const data=await invoke<Result>('codex_usage');setActive({data,updated:new Date().toISOString()})}catch(e){setActive(old=>({...old,loading:false,error:String(e)}))}}
 async function refreshAntigravity(){if(!isTauri())return;setAntigravityLoading(true);try{const email=await invoke<string>('existing_service_account',{service:'Antigravity'});setAntigravityEmail(email);try{const usage=await invoke<AntigravityUsage>('antigravity_usage');setAntigravityUsage(usage.email&&usage.email.toLowerCase()===email.toLowerCase()?usage:null)}catch{setAntigravityUsage(null)}}catch{setAntigravityEmail(null);setAntigravityUsage(null)}finally{setAntigravityLoading(false)}}
 useEffect(()=>{if(!isTauri())return;void refreshAntigravity();const timer=window.setInterval(()=>void refreshAntigravity(),300000);return()=>window.clearInterval(timer)},[]);
 async function refreshProfile(id:string){setProfiles(old=>({...old,[id]:{...old[id],loading:true,error:undefined}}));try{const data=await invoke<Result>('codex_usage',{profile:id});if(!data.account)throw new Error('Compte indisponible. Reconnecte ce compte.');setProfiles(old=>({...old,[id]:{data,updated:new Date().toISOString()}}))}catch(e){setProfiles(old=>({...old,[id]:{...old[id],loading:false,error:String(e)}}))}}
 async function refreshClaude(id:string){setClaudeProfiles(old=>({...old,[id]:{...old[id],loading:true,error:undefined}}));try{const data=await invoke<ClaudeAccount>('claude_account',{profile:id});setClaudeProfiles(old=>({...old,[id]:{data}}))}catch(e){setClaudeProfiles(old=>({...old,[id]:{...old[id],loading:false,error:String(e)}}))}}
 const claudeIds=entries.filter(e=>e.mode==='claude').map(e=>e.id).sort().join(',');
 useEffect(()=>{if(!isTauri())return;const update=()=>claudeIds.split(',').filter(Boolean).forEach(id=>void refreshClaude(id));update();const timer=window.setInterval(update,300000);return()=>window.clearInterval(timer)},[claudeIds]);
 async function openCursorProfile(id:string){setCursorErrors(old=>({...old,[id]:''}));try{await invoke('open_cursor_profile',{profile:id})}catch(e){setCursorErrors(old=>({...old,[id]:String(e)}))}}
 async function connectCursor(){if(!isTauri()){setLoginError('Ouvre l’application Windows pour te connecter.');return}setLoginError('');setLoginBusy(true);const id=crypto.randomUUID();const attempt=++loginAttempt.current;loginProfile.current=id;try{const email=await invoke<string>('login_cursor',{profile:id});if(loginAttempt.current!==attempt)return;setEntries(old=>[...old,{id,service:'Cursor',name:email,remaining:null,reset:'',updated:new Date().toISOString(),mode:'cursor'}]);setShow(false)}catch(e){if(loginAttempt.current===attempt)setLoginError(String(e))}finally{if(loginAttempt.current===attempt){loginProfile.current=null;setLoginBusy(false)}}}
 async function connectClaude(){if(!isTauri()){setLoginError('Ouvre l’application Windows pour te connecter.');return}setLoginError('');setLoginBusy(true);const id=crypto.randomUUID();const attempt=++loginAttempt.current;loginProfile.current=id;try{await invoke('login_claude',{profile:id});if(loginAttempt.current!==attempt)return;const data=await invoke<ClaudeAccount>('claude_account',{profile:id});if(loginAttempt.current!==attempt)return;setClaudeProfiles(old=>({...old,[id]:{data}}));setEntries(old=>[...old,{id,service:'Claude Code',name:data.email||'Compte Claude',remaining:null,reset:'',updated:new Date().toISOString(),mode:'claude'}]);setShow(false)}catch(e){if(loginAttempt.current===attempt)setLoginError(String(e))}finally{if(loginAttempt.current===attempt){loginProfile.current=null;setLoginBusy(false)}}}
 async function disconnectClaude(id:string){setClaudeProfiles(old=>({...old,[id]:{...old[id],loading:true,error:undefined}}));try{await invoke('disconnect_claude',{profile:id});setEntries(old=>old.filter(x=>x.id!==id))}catch(e){setClaudeProfiles(old=>({...old,[id]:{...old[id],loading:false,error:String(e)}}))}}
 async function reconnectClaude(id:string){setClaudeProfiles(old=>({...old,[id]:{...old[id],loading:true,error:undefined}}));try{await invoke('login_claude',{profile:id});await refreshClaude(id)}catch(e){setClaudeProfiles(old=>({...old,[id]:{...old[id],loading:false,error:String(e)}}))}}
 const ids=entries.filter(e=>e.mode==='codex').map(e=>e.id).sort().join(',');
 useEffect(()=>{if(!isTauri())return;const update=()=>{void refreshActive();ids.split(',').filter(Boolean).forEach(id=>void refreshProfile(id))};update();const timer=window.setInterval(update,300000);return()=>window.clearInterval(timer)},[ids]);
 useEffect(()=>{if(!show)return;function onKey(e:KeyboardEvent){if(e.key==='Escape')closeDialog()}window.addEventListener('keydown',onKey);return()=>window.removeEventListener('keydown',onKey)},[show,loginBusy]);
 async function connectCodex(){if(!isTauri()){setLoginError('Ouvre l’application Windows pour te connecter.');return}setLoginError('');setLoginBusy(true);const id=crypto.randomUUID();const attempt=++loginAttempt.current;loginProfile.current=id;try{await invoke('login_codex',{profile:id});if(loginAttempt.current!==attempt)return;setEntries(old=>[...old,{id,service:'Codex',name:'Compte Codex',remaining:null,reset:'',updated:new Date().toISOString(),mode:'codex'}]);setShow(false)}catch(e){if(loginAttempt.current===attempt)setLoginError(String(e))}finally{if(loginAttempt.current===attempt){loginProfile.current=null;setLoginBusy(false)}}}
 async function disconnect(id:string){setProfiles(old=>({...old,[id]:{...old[id],loading:true,error:undefined}}));try{await invoke('disconnect_codex',{profile:id});setEntries(old=>old.filter(x=>x.id!==id))}catch(e){setProfiles(old=>({...old,[id]:{...old[id],loading:false,error:String(e)}}))}}
 async function reconnect(id:string){setProfiles(old=>({...old,[id]:{...old[id],loading:true,error:undefined}}));try{await invoke('login_codex',{profile:id});await refreshProfile(id)}catch(e){setProfiles(old=>({...old,[id]:{...old[id],loading:false,error:String(e)}}))}}
 async function open(service:Service){try{if(isTauri())await invoke('open_usage',{service});else window.open(links[service],'_blank','noopener,noreferrer')}catch{setActive(old=>({...old,error:'Impossible d’ouvrir la page du service.'}))}}
 function openDialog(){setService(page==='Tous'||page==='Reglages'?'Codex':page);setLoginError('');setShow(true)}
 function moveAccount(sourceId:string,targetId:string,after:boolean){
  setEntries(old=>reorderVisibleEntries(old,page,sourceId,targetId,after));
 }
 function cardLayoutTop(element:HTMLElement){
  const visualTop=element.getBoundingClientRect().top;
  const animation=cardAnimations.current.get(element);
  if(!animation||animation.playState==='finished'||animation.playState==='idle')return visualTop;
  const transform=window.getComputedStyle(element).transform;
  if(!transform||transform==='none')return visualTop;
  try{return visualTop-new DOMMatrixReadOnly(transform).m42}catch{return visualTop}
 }
 function startAccountDrag(event:ReactPointerEvent<HTMLElement>,id:string){
  if(event.button!==0||!event.isPrimary||visible.length<2||draggingAccount.current)return;
  const target=event.target as Element;
  if(target.closest('button:not(.account-drag-handle),a,input,textarea,select'))return;
  if(event.pointerType==='touch'&&!target.closest('.account-drag-handle'))return;
  event.preventDefault();
  const card=event.currentTarget;
  const visualTop=card.getBoundingClientRect().top;
  cardAnimations.current.get(card)?.cancel();
  const initialOffset=visualTop-card.getBoundingClientRect().top;
  if(initialOffset)card.style.transform=`translateY(${initialOffset}px)`;
  const rect=card.getBoundingClientRect();
  const drag={id,pointerId:event.pointerId,x:event.clientX,y:event.clientY,offset:initialOffset,moved:false};
  draggingAccount.current=drag;
  const reducedMotion=window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  const positionCard=(clientY:number)=>{
   const layoutTop=card.getBoundingClientRect().top-drag.offset;
   drag.offset=clientY-(drag.y-rect.top)-layoutTop;
   card.style.transform=`translateY(${drag.offset}px)`;
  };
  const onMove=(move:PointerEvent)=>{
   if(move.pointerId!==drag.pointerId)return;
   if(!drag.moved&&Math.hypot(move.clientX-drag.x,move.clientY-drag.y)<5)return;
   if(!drag.moved){drag.moved=true;card.style.pointerEvents='none';setDraggingId(id)}
   move.preventDefault();
   positionCard(move.clientY);
   if(!mainRef.current)return;
   const cards=Array.from(mainRef.current.querySelectorAll<HTMLElement>('.account-list [data-account-id]'));
   // Ignore the visual offset of cards that are still sliding into place.
   // Hit testing their animated position can otherwise swap them back and forth.
   const hit=cards.filter(element=>element!==card).map(element=>{
    const box=element.getBoundingClientRect();
    return {element,box,top:cardLayoutTop(element)};
   }).find(item=>move.clientX>=item.box.left&&move.clientX<=item.box.right&&move.clientY>=item.top&&move.clientY<=item.top+item.box.height);
   const hoveredId=hit?.element.dataset.accountId;
   if(!hit||!hoveredId)return;
   const next=reorderVisibleEntries(entriesRef.current,page,id,hoveredId,move.clientY>=hit.top+hit.box.height/2);
   if(next===entriesRef.current)return;
   const before=new Map(cards.map(element=>[element,{visual:element.getBoundingClientRect().top,layout:cardLayoutTop(element)}]));
   entriesRef.current=next;
   flushSync(()=>setEntries(next));
   positionCard(move.clientY);
   for(const element of cards){
    if(element===card)continue;
    const previous=before.get(element);
    if(!previous||Math.abs(previous.layout-cardLayoutTop(element))<1)continue;
    cardAnimations.current.get(element)?.cancel();
    const delta=previous.visual-element.getBoundingClientRect().top;
    if(!reducedMotion&&Math.abs(delta)>1&&typeof element.animate==='function'){
     cardAnimations.current.set(element,element.animate([{transform:`translateY(${delta}px)`},{transform:'translateY(0)'}],{duration:340,easing:'cubic-bezier(.22,1,.36,1)'}));
    }
   }
  };
  const finish=(up?:PointerEvent)=>{
   if(up&&up.pointerId!==drag.pointerId)return;
   window.removeEventListener('pointermove',onMove);window.removeEventListener('pointerup',onUp);window.removeEventListener('pointercancel',onCancel);window.removeEventListener('blur',onBlur);
   dragCleanup.current=null;
   if(draggingAccount.current!==drag)return;
   card.style.removeProperty('transform');card.style.removeProperty('pointer-events');
   if(!reducedMotion&&typeof card.animate==='function'&&drag.offset)cardAnimations.current.set(card,card.animate([{transform:`translateY(${drag.offset}px)`},{transform:'translateY(0)'}],{duration:260,easing:'cubic-bezier(.22,1,.36,1)'}));
   if(drag.moved)setReorderMessage('Ordre des comptes mis à jour.');
   draggingAccount.current=null;setDraggingId(null);
  };
  const onUp=(up:PointerEvent)=>finish(up);
  const onCancel=(cancel:PointerEvent)=>finish(cancel);
  const onBlur=()=>finish();
  dragCleanup.current=()=>finish();
  window.addEventListener('pointermove',onMove,{passive:false});window.addEventListener('pointerup',onUp);window.addEventListener('pointercancel',onCancel);window.addEventListener('blur',onBlur);
 }
 function moveAccountByKey(id:string,key:string){
  if(key!=='ArrowUp'&&key!=='ArrowDown')return false;
  const visibleAccounts=entries.filter(entry=>page==='Tous'||entry.service===page);
  const index=visibleAccounts.findIndex(entry=>entry.id===id);
  const next=index+(key==='ArrowUp'?-1:1);
  if(index>=0&&next>=0&&next<visibleAccounts.length){
   moveAccount(id,visibleAccounts[next].id,key==='ArrowDown');
   const account=visibleAccounts[index];
   const name=profiles[id]?.data?.account?.email||claudeProfiles[id]?.data?.email||account.name;
   setReorderMessage(`${name}, position ${next+1} sur ${visibleAccounts.length}.`);
  }
  return true;
 }
 const visible=entries.filter(e=>page==='Tous'||e.service===page); const pageTitle=page==='Tous'?'Vue d’ensemble':page==='Reglages'?'Réglages':page==='Codex'?'ChatGPT / Codex':page;
 const count=entries.length;
 const sourceCount=useMemo(()=>new Set(entries.map(e=>e.service)).size,[entries]);
 const lastCodexUpdate=Object.values(profiles).map(p=>p.updated).filter((v):v is string=>Boolean(v)).sort().at(-1);
 return <><div className="titlebar" data-tauri-drag-region><div className="titlebar-label wordmark" data-tauri-drag-region><img className="wordmark-icon" src={brandIcon} alt="" draggable={false}/><span data-tauri-drag-region>Nivra<span className="wordmark-dot">.</span></span></div><div className="titlebar-drag" data-tauri-drag-region/>{isTauri()&&<div className="window-controls"><button className="window-theme" aria-label={appearance.theme==='dark'?'Activer le mode clair':'Activer le mode sombre'} title={appearance.theme==='dark'?'Activer le mode clair':'Activer le mode sombre'} onClick={()=>setAppearance({...appearance,theme:appearance.theme==='dark'?'light':'dark'})}>{appearance.theme==='dark'?<Sun size={15}/>:<Moon size={15}/>}</button><button aria-label="Réduire" title="Réduire" onClick={()=>void getCurrentWindow().minimize()}><Minus size={15}/></button><button aria-label="Agrandir ou restaurer" title="Agrandir ou restaurer" onClick={()=>void getCurrentWindow().toggleMaximize()}><Maximize2 size={12}/></button><button className="window-close" aria-label="Fermer" title="Fermer" onClick={()=>void getCurrentWindow().close()}><X size={15}/></button></div>}</div><Background appearance={appearance}/><div className={`shell ${menuCollapsed?'sidebar-collapsed':''}`}>
  <aside className="sidebar"><nav className="side-nav" aria-label="Navigation principale"><button className={`side-link side-overview ${page==='Tous'?'selected':''}`} onClick={()=>navigate('Tous')} aria-current={page==='Tous'?'page':undefined} aria-label="Vue d’ensemble" title="Vue d’ensemble"><span className="side-link-icon"><img className="overview-icon" src={overviewV2} alt=""/></span><span className="side-link-label">Vue d’ensemble</span>{page==='Tous'&&<span className="side-indicator"/>}</button><div className="side-group-label">Services</div>{services.map(item=><button key={item} className={`side-link ${page===item?'selected':''}`} onClick={()=>navigate(item)} aria-current={page===item?'page':undefined} title={item==='Codex'?'ChatGPT / Codex':item} aria-label={item==='Codex'?'ChatGPT / Codex':item}><BrandMark service={item} small/><span className="side-link-label">{item==='Codex'?'ChatGPT / Codex':item}</span>{page===item&&<span className="side-indicator"/>}</button>)}</nav><div className="sidebar-spacer"/><button className={`side-link settings-link ${page==='Reglages'?'selected':''}`} onClick={()=>navigate('Reglages')} aria-current={page==='Reglages'?'page':undefined} title="Réglages" aria-label="Réglages"><span className="side-link-icon"><img className="custom-menu-icon" src={settingsIcon} alt=""/></span><span className="side-link-label">Réglages</span>{page==='Reglages'&&<span className="side-indicator"/>}</button><button className="side-link collapse-link" onClick={()=>setMenuCollapsed(old=>!old)} aria-expanded={!menuCollapsed} aria-label={menuCollapsed?'Développer le menu':'Réduire le menu'} title={menuCollapsed?'Développer le menu':'Réduire le menu'}><span className="side-link-icon"><img className={`custom-menu-icon ${menuCollapsed?'menu-icon-open':''}`} src={menuIcon} alt=""/></span><span className="side-link-label">Réduire le menu</span></button></aside>
  <main className="main" ref={mainRef}>
   <div key={page} className={`content ${hasNavigated.current?'page-enter':''}`}>{page==='Reglages'?<AppearanceSettings value={appearance} onChange={setAppearance} updater={updater}/>:<>{updater.phase==='ready'&&<div className="update-banner"><span>Nouvelle version {updater.nextVersion} disponible</span><button className="text-button" onClick={()=>navigate('Reglages')}>Voir la mise à jour <ArrowUpRight size={14}/></button></div>}<header className={`page-head ${page==='Tous'?'overview-head':'service-head'}`}><div className="page-heading">{page!=='Tous'&&<div className="page-brand"><BrandMark service={page}/></div>}<div><h1>{pageTitle}</h1>{page==='Tous'&&<p>Tous tes comptes. Une vue claire sur tes limites.</p>}</div></div><button className="button primary" onClick={openDialog}><Plus size={17}/> Ajouter un compte</button></header>
    {page==='Tous'&&<section className="summary" aria-label="Résumé"><div className="summary-cell"><div className="summary-icon"><img className="summary-art accounts-art" src={summaryAccounts} alt=""/></div><div><span>Comptes suivis</span><strong>{count}</strong></div></div><div className="summary-cell"><div className="summary-icon"><img className="summary-art services-art" src={summaryServices} alt=""/></div><div><span>Services</span><strong>{sourceCount}</strong></div></div><div className="summary-cell"><div className="summary-icon"><img className="summary-art clock-art" src={summaryClock} alt=""/></div><div><span>Dernière lecture Codex</span><strong className="time-value">{shortTime(lastCodexUpdate)}</strong></div></div></section>}
    {(page==='Tous'||page==='Antigravity')&&antigravityEmail&&<section className="antigravity-detected" aria-label="Compte Antigravity détecté"><div className="section-header"><div><h2>Compte Antigravity détecté</h2><p>Adresse lue sur cet appareil.</p></div><button className="button ghost" onClick={()=>void refreshAntigravity()} disabled={antigravityLoading}><RefreshCw size={15} className={antigravityLoading?'spin':''}/> Actualiser</button></div><article className="account-card"><div className="account-head"><div className="account-identity"><BrandMark service="Antigravity"/><div className="identity-copy"><div className="account-title">{antigravityEmail} <span className="pill live">Détecté localement</span></div><p>Antigravity · {antigravityUsage?.plan||'offre non identifiée'}</p></div></div></div>{antigravityUsage?.models.length?<div className="antigravity-models">{antigravityUsage.models.filter(m=>m.label&&Number.isFinite(m.remaining)).map((model,i)=>{const percent=Math.round(Math.min(1,Math.max(0,model.remaining))*100);return <div className="quota-window" key={`${model.label}-${i}`}><div className="quota-label"><span>{model.label}</span><span className="quota-number">{percent}<span>%</span></span></div><div className="progress" role="progressbar" aria-label={`Quota restant ${model.label}`} aria-valuemin={0} aria-valuemax={100} aria-valuenow={percent}><span style={{width:`${percent}%`}}/></div><div className="quota-reset"><Clock3 size={12}/><span>{model.reset?`Se renouvelle le ${dateText(model.reset)}`:'Réinitialisation indisponible'}</span></div></div>})}</div>:<p className="form-note"><Info size={14}/> Quotas et offre indisponibles : ouvre Antigravity et actualise. Nivra doit pouvoir vérifier le compte.</p>}</article></section>}
    <div className="section-header accounts-heading"><div><h2>Mes comptes <span className="section-count">{visible.length}</span></h2><p>{page==='Tous'?'Retrouve tes autres comptes et leur disponibilité.':`Les comptes ${page==='Codex'?'ChatGPT / Codex':page} ajoutés à Nivra.`}</p></div><button className="text-button" onClick={openDialog}><Plus size={15}/> Ajouter</button></div>

    {visible.length===0?<div className="empty"><div className="empty-graphic"><div className="empty-orbit">{page==='Tous'?<img src={overviewV2} alt=""/>:<BrandMark service={page}/>}</div><span className="empty-dot one"/><span className="empty-dot two"/></div><div><b>{page==='Tous'?'Tout commence ici':`Aucun compte ${page==='Codex'?'ChatGPT / Codex':page}`}</b><p>{page==='Tous'||page==='Codex'?'Ajoute un compte pour retrouver ses limites dans ton espace.':'Connecte un compte pour l’ajouter à ton espace.'}</p><button className="text-button" onClick={openDialog}>Ajouter un compte <ArrowUpRight size={14}/></button></div></div>:<><div className="reorder-status" role="status" aria-live="polite">{reorderMessage}</div><div className="account-list">{visible.map(entry=>{const claude=entry.mode==='claude';const local=entry.mode==='local';const cursorProfile=entry.mode==='cursor';const state=profiles[entry.id];const claudeState=claudeProfiles[entry.id];const real=entry.mode==='codex';const issue=real?state?.error:claude?claudeState?.error:undefined;const activeEmail=active.data?.account?.email;const profileEmail=state?.data?.account?.email;const isActive=Boolean(real&&activeEmail&&profileEmail&&activeEmail.toLowerCase()===profileEmail.toLowerCase());const connected=real||claude||local||cursorProfile;const lim=state?.data?.limits?.rateLimits;return <article className={`account-card${draggingId===entry.id?' is-dragging':''}`} data-account-id={entry.id} key={entry.id} onPointerDown={e=>startAccountDrag(e,entry.id)} onDragStart={e=>e.preventDefault()}><div className="account-head"><div className="account-identity">{visible.length>1&&<button type="button" className="account-drag-handle" aria-label={`Déplacer ${entry.name}. Utilise les flèches haut et bas pour changer l’ordre.`} title="Glisser pour réorganiser" onKeyDown={e=>{if(moveAccountByKey(entry.id,e.key))e.preventDefault()}}><img src={reorderIcon} alt="" draggable={false}/></button>}<BrandMark service={entry.service}/><div className="identity-copy"><div className="account-title">{(claude?claudeState?.data?.email:state?.data?.account?.email)||entry.name}{issue?<><span className="pill account-warning" role="status" title={issue} aria-label={`${real?'Limites indisponibles':'Connexion à vérifier'} : ${issue}`}><Info size={12}/>{real?'Limites indisponibles':'Connexion à vérifier'}</span><button type="button" className="account-reconnect" onClick={()=>claude?void reconnectClaude(entry.id):void reconnect(entry.id)} disabled={claude?claudeState?.loading:state?.loading}>Reconnecter</button></>:<span className={`pill ${connected?'live':'manual'}`}>{local?'Importé':cursorProfile?'Profil enregistré':connected?'Connecté':'Manuel'}</span>}{isActive&&<span className="pill live">Actif dans Codex</span>}</div><p>{entry.service} <span>·</span> {claude?(claudeState?.data?.subscriptionType||'Compte connecté'):real?(state?.data?.account?.planType||'Compte connecté'):descriptions[entry.service]}</p></div></div><div className="account-actions">{real&&state?.updated&&<span className="account-updated" title={`Dernière actualisation à ${shortTime(state.updated)}`} aria-label={`Dernière actualisation à ${shortTime(state.updated)}`}><Clock3 size={12}/>{shortTime(state.updated)}</span>}{cursorProfile&&<button className="icon-button" title="Ouvrir ce profil Cursor" aria-label="Ouvrir ce profil Cursor" onClick={()=>openCursorProfile(entry.id)}><ArrowUpRight size={16}/></button>}{(real||claude)?<button className="icon-button" title="Actualiser" aria-label={`Actualiser ${entry.name}`} disabled={claude?claudeState?.loading:state?.loading} onClick={()=>claude?void refreshClaude(entry.id):void refreshProfile(entry.id)}><RefreshCw size={15} className={(claude?claudeState?.loading:state?.loading)?'spin':''}/></button>:cursorProfile?null:<button className="icon-button" title="Ouvrir le service" aria-label={`Ouvrir ${entry.service}`} onClick={()=>open(entry.service)}><ArrowUpRight size={16}/></button>}<button className="icon-button danger" title={real||claude?'Déconnecter':'Retirer'} aria-label={`${real||claude?'Déconnecter':'Retirer'} ${entry.name}`} disabled={claude?claudeState?.loading:state?.loading} onClick={()=>claude?void disconnectClaude(entry.id):real?void disconnect(entry.id):setEntries(old=>old.filter(e=>e.id!==entry.id))}><Trash2 size={15}/></button></div></div>{cursorProfile&&cursorErrors[entry.id]&&<div className="notice" role="status"><Info size={15}/><span>{cursorErrors[entry.id]}</span></div>}{cursorProfile?<p className="form-note"><Info size={14}/> Profil Cursor séparé. Les quotas ne sont pas encore disponibles dans Nivra.</p>:local?<p className="form-note"><Info size={14}/> Compte importé depuis {entry.service}. Les quotas ne sont pas encore disponibles dans Nivra.</p>:claude?<p className="form-note"><Info size={14}/> Quotas Claude indisponibles dans Nivra pour le moment.</p>:real?<div className="quota-grid"><Limit value={lim?.primary} label="Limite principale"/><Limit value={lim?.secondary} label="Limite secondaire"/></div>:<div className="manual-values"><div><span>Quota restant</span><strong>{entry.remaining===null?'—':`${entry.remaining} %`}</strong></div><div><span>Réinitialisation</span><strong>{dateText(entry.reset)}</strong></div></div>}</article>})}</div></>}
    </>}
   </div></main>
  {show&&<div className="overlay" onMouseDown={e=>{if(e.target===e.currentTarget)closeDialog()}}><div className="dialog" role="dialog" aria-modal="true" aria-labelledby="dialog-title"><div className="dialog-head"><div><div className="dialog-icon"><BrandMark service={service} small/></div><h2 id="dialog-title">Connecter un compte</h2><p>Choisis un service pour connecter ton compte.</p></div><button className="icon-button" aria-label="Fermer" onClick={closeDialog}><X size={18}/></button></div><div className="service-select"><label htmlFor="service">Service</label><div className="select-wrap"><BrandMark service={service} small/><select id="service" value={service} disabled={loginBusy} onChange={e=>{setService(e.target.value as Service);setLoginError('')}}>{services.map(s=><option key={s}>{s}</option>)}</select><ChevronDown size={15}/></div></div>{service==='Cursor'?<div className="connect-box"><div className="connect-icon"><Link2 size={17}/></div><div><h3>Connecter avec Cursor</h3><p>Une nouvelle fenêtre Cursor s’ouvre avec un profil séparé. Connecte-toi dans cette fenêtre : Nivra ajoutera le compte automatiquement.</p><p className="security-note">Tu peux répéter l’opération pour un autre compte. Les quotas ne sont pas encore disponibles dans Nivra.</p><button type="button" className="button primary connect-button" onClick={connectCursor} disabled={loginBusy}>{loginBusy?<><LoaderCircle size={15} className="spin"/> Connecte-toi dans Cursor…</>:<>Connecter le compte <ArrowUpRight size={15}/></>}</button>{loginError&&<p className="error-copy" role="alert">{loginError}</p>}</div></div>:service==='Claude Code'?<div className="connect-box"><div className="connect-icon"><Link2 size={17}/></div><div><h3>Connecter avec Claude</h3><p>Connecte-toi dans le navigateur avec Claude Code. Ton adresse sera affichée si elle est disponible.</p><p className="security-note">Claude Code conserve les identifiants de ce profil localement. Les quotas ne sont pas encore disponibles dans Nivra.</p><button type="button" className="button primary connect-button" onClick={connectClaude} disabled={loginBusy}>{loginBusy?<><LoaderCircle size={15} className="spin"/> En attente du navigateur…</>:<>Connecter le compte <ArrowUpRight size={15}/></>}</button>{loginError&&<p className="error-copy" role="alert">{loginError}</p>}</div></div>:service==='Codex'?<div className="connect-box"><div className="connect-icon"><Link2 size={17}/></div><div><h3>Connecter avec ChatGPT <span className="pill live">Automatique</span></h3><p>Connecte-toi dans ton navigateur : ton adresse e-mail et tes quotas disponibles apparaîtront dans Nivra.</p><p className="security-note">Si le coffre Windows refuse le jeton, Codex l’enregistre dans un fichier local non chiffré de ton profil Windows.</p><button type="button" className="button primary connect-button" onClick={connectCodex} disabled={loginBusy}>{loginBusy?<><LoaderCircle size={15} className="spin"/> En attente du navigateur…</>:<>Connecter le compte <ArrowUpRight size={15}/></>}</button>{loginError&&<p className="error-copy" role="alert">{loginError}</p>}</div></div>:<div className="connect-box"><div className="connect-icon"><Info size={17}/></div><div><h3>Connexion Antigravity indisponible</h3><p>La connexion permettant de choisir un compte Google dans Nivra n’est pas encore prête.</p></div></div>}<div className="dialog-actions"><button type="button" className="button secondary" onClick={closeDialog}>{loginBusy?'Annuler':'Fermer'}</button></div></div></div>}
 </div></>;
}
const preview = new URLSearchParams(window.location.search).get('preview') === 'ribbon-glow';
createRoot(document.getElementById('root')!).render(preview ? <RibbonGlow background="#0B0A10" color1="#A67CFF" color2="#4B1CBF" style={{position:'fixed',inset:0,width:'100%',height:'100%',minWidth:0,minHeight:0}}/> : <App/>);
