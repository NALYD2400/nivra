import addAccountAction from './assets/add-account-action.png';
import refreshAction from './assets/refresh-action.png';
import { AccountConnection } from './AccountConnection';
import { useEffect, useMemo, useRef, useState, type FormEvent, type PointerEvent as ReactPointerEvent } from 'react';
import { createRoot } from 'react-dom/client';
import { flushSync } from 'react-dom';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { ArrowDownLeft, ArrowUpRight, ChevronDown, Clock3, ExternalLink, Info, Link2, LoaderCircle, Maximize2, Minus, Moon, PanelLeftClose, PanelLeftOpen, RefreshCw, Settings2, Sparkles, Sun, Trash2, X } from 'lucide-react';
import brandIcon from './assets/nivra-taskbar.png';
import summaryAccounts from './assets/summary-accounts.png';
import gatewayIcon from './assets/summary-services.png';
import summaryServices from './assets/summary-services-v2.png';
import summaryClock from './assets/summary-clock.png';
import reorderIcon from './assets/reorder-icon.png';
import forecastIcon from './assets/forecast-icon.png';
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
import { Forecast } from './Forecast';
import { Gateway } from './Gateway';
import { AntigravityCliModels, accountsKey, readAccounts, type GatewayAccount } from './AntigravityCliModels';
import { upsertAntigravityAccount } from './antigravity-accounts';
import { matchAntigravityUsage, parseAntigravityUsages, type AntigravityUsage } from './antigravity-usage';
import { readUsageHistory, recordUsage } from './usage-forecast';
import RibbonGlow from './components/originkit/ui/ribbon-glow';
import './originkit.css';
import './style.css';

type Service = 'Codex' | 'Claude Code' | 'Cursor' | 'Antigravity';
type Entry = { id:string; service:Service; name:string; remaining:number|null; reset:string; updated:string; mode?:'codex'|'claude'|'local'|'cursor'|'antigravity' };
type WindowLimit = { usedPercent:number; windowDurationMins:number|null; resetsAt:number|null };
type Result = { account:{email?:string; type?:string; planType?:string}|null; limits:{rateLimits?:{primary?:WindowLimit|null; secondary?:WindowLimit|null}} };
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
function AntigravityLimits({usage}:{usage:AntigravityUsage|null}) {
 const models=usage?.models.filter(model=>model.label&&Number.isFinite(model.remaining))??[];
 const windows=usage?.windows??[];
 if(!models.length&&!windows.length)return <p className="form-note"><Info size={14}/> Quotas indisponibles : ouvre ce compte dans Antigravity puis actualise. Le compte doit être identifié sans ambiguïté.</p>;
 const card=(label:string,remaining:number,reset:string|undefined,key:string)=><div className="quota-window" key={key}><div className="quota-label"><span>{label}</span><span className="quota-number">{Math.round(remaining*100)}<span>%</span></span></div><div className="progress" role="progressbar" aria-label={`Quota restant ${label}`} aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(remaining*100)}><span style={{width:`${remaining*100}%`}}/></div><div className="quota-reset"><Clock3 size={12}/><span>{reset?`Se renouvelle le ${dateText(reset)}`:'Réinitialisation indisponible'}</span></div></div>;
 return <>{windows.length>0&&<div className="antigravity-models">{windows.map((window,index)=>card(`${window.group} · ${window.window==='5h'?'5 heures':'semaine'}`,window.remaining,window.reset,`${window.group}-${window.window}-${index}`))}</div>}{models.length>0&&(windows.length>0?<details className="codex-models"><summary>Disponibilité par modèle</summary><div className="antigravity-models">{models.map((model,index)=>card(model.label,model.remaining,model.reset,`${model.label}-${index}`))}</div></details>:<div className="antigravity-models">{models.map((model,index)=>card(model.label,model.remaining,model.reset,`${model.label}-${index}`))}</div>)}</>;
}

function App() {
 const updater=useNivraUpdater();
 const [entries,setEntries]=useState<Entry[]>(()=>{try{const parsed=JSON.parse(localStorage.getItem('quota-entries')||'[]');return Array.isArray(parsed)?parsed:[]}catch{return []}});
 const [active,setActive]=useState<Snapshot>({});
 const [antigravityEmail,setAntigravityEmail]=useState<string|null>(null);
 const [antigravityConnectionNotice,setAntigravityConnectionNotice]=useState('');
 const [antigravityLoading,setAntigravityLoading]=useState(false);
 const [antigravitySamples,setAntigravitySamples]=useState<AntigravityUsage[]>([]);
 const [antigravityUpdated,setAntigravityUpdated]=useState<string|null>(null);
 const [antigravityError,setAntigravityError]=useState(false);
 const antigravityRefreshInFlight=useRef(false);
 const antigravityUsage=matchAntigravityUsage(antigravityEmail,antigravitySamples);
 const [profiles,setProfiles]=useState<Record<string,Snapshot>>({});
 const [codexModels,setCodexModels]=useState<Record<string,{loading:boolean;items?:{id:string;name:string;is_default:boolean}[];error?:string}>>({});
 const [usageHistory,setUsageHistory]=useState(readUsageHistory);
 useEffect(()=>{try{localStorage.setItem('nivra-usage-history-v1',JSON.stringify(usageHistory))}catch{/* Le suivi reste disponible pour cette session. */}},[usageHistory]);
 const [cursorErrors,setCursorErrors]=useState<Record<string,string>>({});
 const [claudeProfiles,setClaudeProfiles]=useState<Record<string,{data?:ClaudeAccount; error?:string; loading?:boolean}>>({});
 const [page,setPage]=useState<Service|'Tous'|'Reglages'|'Previsions'|'Passerelle'>('Tous');
 const [gatewayAccountCount,setGatewayAccountCount]=useState(()=>readAccounts().length);
 useEffect(()=>{const refresh=()=>setGatewayAccountCount(readAccounts().length);window.addEventListener('nivra-antigravity-accounts-changed',refresh);return()=>window.removeEventListener('nivra-antigravity-accounts-changed',refresh)},[]);
 const draggingAccount=useRef<{id:string;pointerId:number;x:number;y:number;offset:number;moved:boolean}|null>(null);
 const dragCleanup=useRef<(()=>void)|null>(null);
 const cardAnimations=useRef(new WeakMap<HTMLElement,Animation>());
 const entriesRef=useRef(entries);entriesRef.current=entries;
 const [draggingId,setDraggingId]=useState<string|null>(null);
 useEffect(()=>()=>{dragCleanup.current?.()},[]);
 const [reorderMessage,setReorderMessage]=useState('');
 const hasNavigated=useRef(false);
 const mainRef=useRef<HTMLElement>(null);
 function navigate(next:Service|'Tous'|'Reglages'|'Previsions'|'Passerelle'){if(next===page)return;hasNavigated.current=true;mainRef.current?.scrollTo({top:0,behavior:'instant'});setPage(next)}
 const [menuCollapsed,setMenuCollapsed]=useState(()=>localStorage.getItem('nivra-menu-collapsed')==='true');
 useEffect(()=>{localStorage.setItem('nivra-menu-collapsed',String(menuCollapsed))},[menuCollapsed]);
 const [appearance,setAppearance]=useState<Appearance>(readAppearance);
 useEffect(()=>{setAppearance(old=>old.ribbonRevision===2?old:{...old,accent:'violet',glow:true,glowIntensity:100,ribbonRevision:2})},[]);
 useEffect(()=>{const root=document.documentElement;root.dataset.theme=appearance.theme;root.dataset.accent=appearance.accent;root.dataset.density=appearance.density;localStorage.setItem('nivra-appearance',JSON.stringify(appearance))},[appearance]);
 const loginAttempt=useRef(0); const loginProfile=useRef<string|null>(null);
 function closeDialog(){if(service==='Antigravity'&&loginBusy&&isTauri())void invoke('antigravity_gateway_cancel');loginAttempt.current++;const profile=loginProfile.current;loginProfile.current=null;if(profile&&isTauri())void invoke('cancel_login',{profile});setLoginBusy(false);setShow(false)}
 const [show,setShow]=useState(false); const [service,setService]=useState<Service>('Codex');
 const [loginBusy,setLoginBusy]=useState(false); const [loginError,setLoginError]=useState('');
 useEffect(()=>{localStorage.setItem('quota-entries',JSON.stringify(entries))},[entries]);
 async function refreshActive(){setActive(old=>({...old,loading:true,error:undefined}));try{if(!isTauri())throw new Error('Ouvre l’application Windows pour consulter les quotas.');const data=await invoke<Result>('codex_usage');setActive({data,updated:new Date().toISOString()})}catch(e){setActive(old=>({...old,loading:false,error:String(e)}))}}
 async function refreshAntigravity(){if(!isTauri()||antigravityRefreshInFlight.current)return;antigravityRefreshInFlight.current=true;setAntigravityLoading(true);try{const [email,usage]=await Promise.allSettled([invoke<string>('existing_service_account',{service:'Antigravity'}),invoke<unknown>('antigravity_usage')]);setAntigravityEmail(email.status==='fulfilled'?email.value:null);if(usage.status==='fulfilled'&&Array.isArray(usage.value)){setAntigravitySamples(parseAntigravityUsages(usage.value));setAntigravityUpdated(new Date().toISOString());setAntigravityError(false)}else setAntigravityError(true)}finally{antigravityRefreshInFlight.current=false;setAntigravityLoading(false)}}
 useEffect(()=>{if(!isTauri())return;void refreshAntigravity();const timer=window.setInterval(()=>void refreshAntigravity(),300000);return()=>window.clearInterval(timer)},[]);
 async function refreshProfile(id:string){setProfiles(old=>({...old,[id]:{...old[id],loading:true,error:undefined}}));try{const data=await invoke<Result>('codex_usage',{profile:id});if(!data.account)throw new Error('Compte indisponible. Reconnecte ce compte.');setProfiles(old=>({...old,[id]:{data,updated:new Date().toISOString()}}));setUsageHistory(old=>recordUsage(old,id,data.limits?.rateLimits,Date.now()))}catch(e){setProfiles(old=>({...old,[id]:{...old[id],loading:false,error:String(e)}}))}}
 async function loadCodexModels(id:string){setCodexModels(old=>({...old,[id]:{loading:true}}));try{const items=await invoke<{id:string;name:string;is_default:boolean}[]>('codex_profile_models',{profile:id});setCodexModels(old=>({...old,[id]:{loading:false,items}}))}catch(error){setCodexModels(old=>({...old,[id]:{loading:false,error:String(error)}}))}}
 async function refreshClaude(id:string){setClaudeProfiles(old=>({...old,[id]:{...old[id],loading:true,error:undefined}}));try{const data=await invoke<ClaudeAccount>('claude_account',{profile:id});setClaudeProfiles(old=>({...old,[id]:{data}}))}catch(e){setClaudeProfiles(old=>({...old,[id]:{...old[id],loading:false,error:String(e)}}))}}
 const claudeIds=entries.filter(e=>e.mode==='claude').map(e=>e.id).sort().join(',');
 useEffect(()=>{if(!isTauri())return;const update=()=>claudeIds.split(',').filter(Boolean).forEach(id=>void refreshClaude(id));update();const timer=window.setInterval(update,300000);return()=>window.clearInterval(timer)},[claudeIds]);
 async function openClaudeProfile(id:string){setCursorErrors(old=>({...old,[id]:''}));try{await invoke('open_claude_profile',{profile:id})}catch(e){setCursorErrors(old=>({...old,[id]:String(e)}))}}
 async function openCodexProfile(id:string){setCursorErrors(old=>({...old,[id]:''}));try{await invoke('open_codex_profile',{profile:id})}catch(e){setCursorErrors(old=>({...old,[id]:String(e)}))}}
 async function openCursorProfile(id:string){setCursorErrors(old=>({...old,[id]:''}));try{await invoke('open_cursor_profile',{profile:id})}catch(e){setCursorErrors(old=>({...old,[id]:String(e)}))}}
 async function openAntigravityProfile(id:string){setCursorErrors(old=>({...old,[id]:''}));try{await invoke('open_antigravity_profile',{profile:id})}catch(e){setCursorErrors(old=>({...old,[id]:String(e)}))}}
 async function connectAntigravity(){
  if(!isTauri()){setLoginError('Ouvre l’application Windows pour te connecter.');return}
  setLoginError('');setLoginBusy(true);const attempt=++loginAttempt.current;let account:GatewayAccount|null=null;
  try{
   account=await invoke<GatewayAccount>('antigravity_gateway_login');
   if(loginAttempt.current!==attempt){await invoke('gateway_forget_key',{reference:account.credential_ref});account=null;return}
   const {next,previous}=upsertAntigravityAccount(readAccounts(),account);
   localStorage.setItem(accountsKey,JSON.stringify(next));const current=account;account=null;
   let notice=previous?'Connexion du compte Google mise à jour. Si la passerelle utilisait ce compte, arrête-la puis réactive-le.':'Compte Google ajouté.';
   if(previous&&previous.credential_ref!==current.credential_ref){try{await invoke('gateway_forget_key',{reference:previous.credential_ref})}catch{notice+=' L’ancienne connexion locale n’a pas pu être retirée du coffre Windows.'}}
   setAntigravityConnectionNotice(notice);window.dispatchEvent(new Event('nivra-antigravity-accounts-changed'));setShow(false);navigate('Antigravity');
  }catch(e){if(account){try{await invoke('gateway_forget_key',{reference:account.credential_ref})}catch{/* Le compte peut être retiré du coffre Windows. */}}if(loginAttempt.current===attempt)setLoginError(String(e))}finally{if(loginAttempt.current===attempt)setLoginBusy(false)}
 }
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
 function openDialog(){setService(page==='Tous'||page==='Reglages'||page==='Previsions'||page==='Passerelle'?'Codex':page);setLoginError('');setShow(true)}
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
  <aside className="sidebar"><nav className="side-nav" aria-label="Navigation principale"><button className={`side-link side-overview ${page==='Tous'?'selected':''}`} onClick={()=>navigate('Tous')} aria-current={page==='Tous'?'page':undefined} aria-label="Vue d’ensemble" title="Vue d’ensemble"><span className="side-link-icon"><img className="overview-icon" src={overviewV2} alt=""/></span><span className="side-link-label">Vue d’ensemble</span>{page==='Tous'&&<span className="side-indicator"/>}</button><button className={`side-link forecast-link ${page==='Previsions'?'selected':''}`} onClick={()=>navigate('Previsions')} aria-current={page==='Previsions'?'page':undefined} aria-label="Prévisions" title="Prévisions"><span className="side-link-icon forecast-nav"><img src={forecastIcon} alt=""/></span><span className="side-link-label">Prévisions</span></button><button className={`side-link ${page==='Passerelle'?'selected':''}`} onClick={()=>navigate('Passerelle')} aria-current={page==='Passerelle'?'page':undefined} aria-label="Passerelle" title="Passerelle"><span className="side-link-icon gateway-nav-icon"><img src={gatewayIcon} alt=""/></span><span className="side-link-label">Passerelle</span></button><div className="side-group-label">Services</div>{services.map(item=><button key={item} className={`side-link ${page===item?'selected':''}`} onClick={()=>navigate(item)} aria-current={page===item?'page':undefined} title={item==='Codex'?'ChatGPT / Codex':item} aria-label={item==='Codex'?'ChatGPT / Codex':item}><BrandMark service={item} small/><span className="side-link-label">{item==='Codex'?'ChatGPT / Codex':item}</span>{page===item&&<span className="side-indicator"/>}</button>)}</nav><div className="sidebar-spacer"/><button className={`side-link settings-link ${page==='Reglages'?'selected':''}`} onClick={()=>navigate('Reglages')} aria-current={page==='Reglages'?'page':undefined} title="Réglages" aria-label="Réglages"><span className="side-link-icon"><img className="custom-menu-icon" src={settingsIcon} alt=""/></span><span className="side-link-label">Réglages</span>{page==='Reglages'&&<span className="side-indicator"/>}</button><button className="side-link collapse-link" onClick={()=>setMenuCollapsed(old=>!old)} aria-expanded={!menuCollapsed} aria-label={menuCollapsed?'Développer le menu':'Réduire le menu'} title={menuCollapsed?'Développer le menu':'Réduire le menu'}><span className="side-link-icon"><img className={`custom-menu-icon ${menuCollapsed?'menu-icon-open':''}`} src={menuIcon} alt=""/></span><span className="side-link-label">Réduire le menu</span></button></aside>
  <main className="main" ref={mainRef}>
   <div key={page} className={`content ${hasNavigated.current?'page-enter':''}`}>{page==='Passerelle'?<Gateway/>:page==='Reglages'?<AppearanceSettings value={appearance} onChange={setAppearance} updater={updater}/>:page==='Previsions'?<Forecast accounts={entries.filter(entry=>entry.mode==='codex'||entry.mode==='claude').map(entry=>({id:entry.id,service:entry.mode==='claude'?'Claude Code' as const:'Codex' as const,name:profiles[entry.id]?.data?.account?.email||claudeProfiles[entry.id]?.data?.email||entry.name}))} history={usageHistory} icon={forecastIcon} onOpenAccounts={()=>navigate('Codex')}/>:<>{updater.phase==='ready'&&<div className="update-banner"><span>Nouvelle version {updater.nextVersion} disponible</span><button className="text-button" onClick={()=>navigate('Reglages')}>Voir la mise à jour <ArrowUpRight size={14}/></button></div>}<header className={`page-head ${page==='Tous'?'overview-head':'service-head'}`}><div className="page-heading">{page==='Tous'?<img className="overview-heading-icon" src={overviewV2} alt=""/>:<div className="page-brand"><BrandMark service={page}/></div>}<div><h1>{pageTitle}</h1>{page==='Tous'&&<p>Tous tes comptes. Une vue claire sur tes limites.</p>}</div></div><button className="button primary action-button" aria-label="Ajouter un compte" onClick={openDialog}><span className="action-button-icon" aria-hidden="true"><img src={addAccountAction} alt=""/></span><span>Ajouter un compte</span></button></header>
    {page==='Tous'&&<section className="summary" aria-label="Résumé"><div className="summary-cell"><div className="summary-icon"><img className="summary-art accounts-art" src={summaryAccounts} alt=""/></div><div><span>Comptes suivis</span><strong>{count}</strong></div></div><div className="summary-cell"><div className="summary-icon"><img className="summary-art services-art" src={summaryServices} alt=""/></div><div><span>Services</span><strong>{sourceCount}</strong></div></div><div className="summary-cell"><div className="summary-icon"><img className="summary-art clock-art" src={summaryClock} alt=""/></div><div><span>Dernière lecture Codex</span><strong className="time-value">{shortTime(lastCodexUpdate)}</strong></div></div></section>}
    {(page==='Tous'||page==='Antigravity')&&antigravityEmail&&<section className="antigravity-detected" aria-label="Compte Antigravity détecté"><div className="section-header"><div><h2>Compte Antigravity détecté</h2><p>Adresse lue sur cet appareil.</p></div><button className="button ghost action-button" onClick={()=>void refreshAntigravity()} disabled={antigravityLoading}><span className="action-button-icon" aria-hidden="true"><img src={refreshAction} alt="" className={antigravityLoading?'spin':''}/></span><span>Actualiser</span></button></div><article className="account-card"><div className="account-head"><div className="account-identity"><BrandMark service="Antigravity"/><div className="identity-copy"><div className="account-title">{antigravityEmail} <span className="pill live">Détecté localement</span></div><p>Antigravity · {antigravityUsage?.plan||'offre non identifiée'}</p></div></div></div><AntigravityLimits usage={antigravityUsage}/></article></section>}
    {(page==='Tous'||page==='Antigravity')&&antigravityError&&<p className="form-note" role="status"><Info size={14}/>{antigravityUpdated?`Actualisation Antigravity impossible. Les derniers quotas affichés datent de ${shortTime(antigravityUpdated)} et peuvent avoir changé.`:'Actualisation des quotas Antigravity impossible. Ouvre Antigravity et réessaie.'}</p>}
    {page==='Antigravity'&&antigravityConnectionNotice&&<p className="gateway-message" role="status">{antigravityConnectionNotice}</p>}{page==='Antigravity'&&<AntigravityCliModels/>}
    <div className="section-header accounts-heading"><div><h2>Mes comptes <span className="section-count">{visible.length}</span></h2><p>{page==='Tous'?'Retrouve tes autres comptes et leur disponibilité.':`Les comptes ${page==='Codex'?'ChatGPT / Codex':page} ajoutés à Nivra.`}</p></div></div>

    {visible.length===0&&page==='Antigravity'&&gatewayAccountCount>0?null:visible.length===0?<div className="empty"><div className="empty-graphic"><div className="empty-orbit">{page==='Tous'?<img src={overviewV2} alt=""/>:<BrandMark service={page}/>}</div><span className="empty-dot one"/><span className="empty-dot two"/></div><div><b>{page==='Tous'?'Tout commence ici':`Aucun compte ${page==='Codex'?'ChatGPT / Codex':page}`}</b><p>{page==='Tous'||page==='Codex'?'Ajoute un compte pour retrouver ses limites dans ton espace.':'Connecte un compte pour l’ajouter à ton espace.'}</p><button className="text-button" onClick={openDialog}>Ajouter un compte <ArrowUpRight size={14}/></button></div></div>:<><div className="reorder-status" role="status" aria-live="polite">{reorderMessage}</div><div className="account-list">{visible.map(entry=>{const claude=entry.mode==='claude';const local=entry.mode==='local';const cursorProfile=entry.mode==='cursor';const antigravityProfile=entry.mode==='antigravity';const antigravityProfileUsage=antigravityProfile?matchAntigravityUsage(entry.name,antigravitySamples):null;const state=profiles[entry.id];const claudeState=claudeProfiles[entry.id];const real=entry.mode==='codex';const issue=real?state?.error:claude?claudeState?.error:undefined;const activeEmail=active.data?.account?.email;const profileEmail=state?.data?.account?.email;const isActive=Boolean(real&&activeEmail&&profileEmail&&activeEmail.toLowerCase()===profileEmail.toLowerCase());const connected=real||claude||local||cursorProfile||antigravityProfile;const lim=state?.data?.limits?.rateLimits;return <article className={`account-card${draggingId===entry.id?' is-dragging':''}`} data-account-id={entry.id} key={entry.id} onPointerDown={e=>startAccountDrag(e,entry.id)} onDragStart={e=>e.preventDefault()}><div className="account-head"><div className="account-identity">{visible.length>1&&<button type="button" className="account-drag-handle" aria-label={`Déplacer ${entry.name}. Utilise les flèches haut et bas pour changer l’ordre.`} title="Glisser pour réorganiser" onKeyDown={e=>{if(moveAccountByKey(entry.id,e.key))e.preventDefault()}}><img src={reorderIcon} alt="" draggable={false}/></button>}<BrandMark service={entry.service}/><div className="identity-copy"><div className="account-title">{(claude?claudeState?.data?.email:state?.data?.account?.email)||entry.name}{issue?<><span className="pill account-warning" role="status" title={issue} aria-label={`${real?'Limites indisponibles':'Connexion à vérifier'} : ${issue}`}><Info size={12}/>{real?'Limites indisponibles':'Connexion à vérifier'}</span><button type="button" className="account-reconnect" onClick={()=>claude?void reconnectClaude(entry.id):void reconnect(entry.id)} disabled={claude?claudeState?.loading:state?.loading}>Reconnecter</button></>:<span className={`pill ${connected?'live':'manual'}`}>{local?'Importé':(cursorProfile||antigravityProfile)?'Profil enregistré':connected?'Connecté':'Manuel'}</span>}{isActive&&<span className="pill live">Actif dans Codex</span>}</div><p>{entry.service} <span>·</span> {claude?(claudeState?.data?.subscriptionType||'Compte connecté'):real?(state?.data?.account?.planType||'Compte connecté'):antigravityProfile?(antigravityProfileUsage?.plan||'Profil enregistré'):descriptions[entry.service]}</p></div></div><div className="account-actions">{real&&state?.updated&&<span className="account-updated" title={`Dernière actualisation à ${shortTime(state.updated)}`} aria-label={`Dernière actualisation à ${shortTime(state.updated)}`}><Clock3 size={12}/>{shortTime(state.updated)}</span>}{claude&&<button className="icon-button" title="Ouvrir Claude Code avec ce compte" aria-label={`Ouvrir Claude Code avec ${entry.name}`} onClick={()=>void openClaudeProfile(entry.id)}><ArrowUpRight size={16}/></button>}{real&&<button className="icon-button" title="Ouvrir Codex avec ce compte" aria-label={`Ouvrir Codex avec ${entry.name}`} onClick={()=>void openCodexProfile(entry.id)}><ArrowUpRight size={16}/></button>}{(cursorProfile||antigravityProfile)&&<button className="icon-button" title={`Ouvrir ce profil ${entry.service}`} aria-label={`Ouvrir ce profil ${entry.service}`} onClick={()=>antigravityProfile?openAntigravityProfile(entry.id):openCursorProfile(entry.id)}><ArrowUpRight size={16}/></button>}{antigravityProfile&&<button className="icon-button" title="Actualiser les quotas" aria-label={`Actualiser ${entry.name}`} disabled={antigravityLoading} onClick={()=>void refreshAntigravity()}><RefreshCw size={15} className={antigravityLoading?'spin':''}/></button>}{(real||claude)?<button className="icon-button" title="Actualiser" aria-label={`Actualiser ${entry.name}`} disabled={claude?claudeState?.loading:state?.loading} onClick={()=>claude?void refreshClaude(entry.id):void refreshProfile(entry.id)}><RefreshCw size={15} className={(claude?claudeState?.loading:state?.loading)?'spin':''}/></button>:(cursorProfile||antigravityProfile)?null:<button className="icon-button" title="Ouvrir le service" aria-label={`Ouvrir ${entry.service}`} onClick={()=>open(entry.service)}><ArrowUpRight size={16}/></button>}<button className="icon-button danger" title={real||claude?'Déconnecter':'Retirer'} aria-label={`${real||claude?'Déconnecter':'Retirer'} ${entry.name}`} disabled={claude?claudeState?.loading:state?.loading} onClick={()=>claude?void disconnectClaude(entry.id):real?void disconnect(entry.id):setEntries(old=>old.filter(e=>e.id!==entry.id))}><Trash2 size={15}/></button></div></div>{(real||claude||cursorProfile||antigravityProfile)&&cursorErrors[entry.id]&&<div className="notice" role="status"><Info size={15}/><span>{cursorErrors[entry.id]}</span></div>}{antigravityProfile?<AntigravityLimits usage={antigravityProfileUsage}/>:cursorProfile?<p className="form-note"><Info size={14}/> Profil Cursor séparé. Les quotas ne sont pas encore disponibles dans Nivra.</p>:local?<p className="form-note"><Info size={14}/> Compte importé depuis {entry.service}. Les quotas ne sont pas encore disponibles dans Nivra.</p>:claude?<p className="form-note"><Info size={14}/> Quotas Claude indisponibles dans Nivra pour le moment.</p>:real?<><div className="quota-grid"><Limit value={lim?.primary} label="Limite principale"/><Limit value={lim?.secondary} label="Limite secondaire"/></div><details className="codex-models"><summary onClick={()=>{if(!codexModels[entry.id])void loadCodexModels(entry.id)}}>Modèles proposés par Codex</summary><p>Catalogue de Codex : la présence d’un modèle ne garantit pas son accès avec ce compte.</p>{codexModels[entry.id]?.loading?<p>Chargement…</p>:codexModels[entry.id]?.error?<p role="status">{codexModels[entry.id].error}</p>:codexModels[entry.id]?.items?.length?<ul>{codexModels[entry.id].items?.map(model=><li key={model.id}>{model.name} <code>{model.id}</code>{model.is_default?' · Par défaut':''}</li>)}</ul>:<p>Aucun modèle communiqué.</p>}<button type="button" className="button ghost" disabled={codexModels[entry.id]?.loading} onClick={()=>void loadCodexModels(entry.id)}>Actualiser les modèles</button></details></>:<div className="manual-values"><div><span>Quota restant</span><strong>{entry.remaining===null?'—':`${entry.remaining} %`}</strong></div><div><span>Réinitialisation</span><strong>{dateText(entry.reset)}</strong></div></div>}</article>})}</div></>}
    </>}
   </div></main>
  {show&&<AccountConnection service={service} busy={loginBusy} error={loginError} onService={next=>{setService(next);setLoginError('')}} onClose={closeDialog} onConnect={()=>void (service==='Codex'?connectCodex():service==='Claude Code'?connectClaude():service==='Antigravity'?connectAntigravity():connectCursor())} renderBrand={item=><BrandMark service={item} small/>}/>}
 </div></>;
}
const preview = new URLSearchParams(window.location.search).get('preview') === 'ribbon-glow';
createRoot(document.getElementById('root')!).render(preview ? <RibbonGlow background="#0B0A10" color1="#A67CFF" color2="#4B1CBF" style={{position:'fixed',inset:0,width:'100%',height:'100%',minWidth:0,minHeight:0}}/> : <App/>);
