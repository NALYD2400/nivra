import {useEffect,useRef,useState,type ReactNode,type PointerEvent} from 'react';
import {invoke,isTauri} from '@tauri-apps/api/core';
import {Clock3,RefreshCw,Info,Trash2} from 'lucide-react';
import './cursor-grok.css';
import {subscriptionPlan} from './subscription-plan';
import type {CursorUsage} from './quota-forecast';

type Usage=CursorUsage;
const enabledKey='nivra-cursor-quota-enabled';
export function useCursorQuota(){
 const [data,setData]=useState<Usage|null>(null),[error,setError]=useState(''),[loading,setLoading]=useState(false);
 const [updated,setUpdated]=useState<string>();
 const [enabled,setEnabled]=useState(()=>localStorage.getItem(enabledKey)==='true');
 const running=useRef(false);
 const generation=useRef(0);
 const lastError=useRef('');
 async function refresh(profile?:string){
  if(running.current)return false;
  if(!isTauri()){lastError.current='Ouvre l’application Windows pour lire ton quota.';setError(lastError.current);return false}
  const attempt=++generation.current;
  running.current=true;setLoading(true);setError('');lastError.current='';
  try{const result=await invoke<Usage>(profile?'login_cursor':'cursor_usage',profile?{profile}:{});if(attempt!==generation.current)return false;setData(result);setUpdated(new Date().toISOString());setEnabled(true);localStorage.setItem(enabledKey,'true');return true}
  catch(error){if(attempt===generation.current){lastError.current=String(error);setError(lastError.current)}return false}finally{if(attempt===generation.current){setLoading(false);running.current=false}}
 }
 useEffect(()=>{if(!enabled)return;if(!data)void refresh();const timer=setInterval(()=>void refresh(),300000);return()=>clearInterval(timer)},[enabled]);
 function cancelPending(){generation.current++;running.current=false;setLoading(false)}
 function forget(){cancelPending();setEnabled(false);setData(null);setUpdated(undefined);setError('');localStorage.removeItem(enabledKey)}
 return {data,error,loading,enabled,updated,refresh,forget,lastError,cancelPending};
}
function reset(value:string|null){if(!value)return 'Renouvellement non communiqué';const date=new Date(value);return Number.isNaN(date.getTime())?'Renouvellement non communiqué':`Renouvellement : ${date.toLocaleString('fr-FR')}`}
export function CursorGrok({quota,onConnect,brand,compact=false,grok=false,dragHandle,accountId,dragging=false,onPointerDown}:{quota:ReturnType<typeof useCursorQuota>;onConnect:()=>void;brand:ReactNode;compact?:boolean;grok?:boolean;dragHandle?:ReactNode;accountId?:string;dragging?:boolean;onPointerDown?:(event:PointerEvent<HTMLElement>)=>void}){
 const {data,error,loading,updated}=quota;
 const windows=grok&&data?.unified!==true?data?.windows.filter(window=>window.kind==='grok-bot'):data?.windows;
 const service=data?.unified?'Cursor · xAI / Grok':grok?'xAI / Grok':'Cursor';
 const time=updated?new Date(updated).toLocaleTimeString('fr-FR',{hour:'2-digit',minute:'2-digit'}):null;
 const card=<article className={`account-card cursor-account-card${dragging?' is-dragging':''}`} aria-label={`Compte ${service}`} data-account-id={accountId} onPointerDown={onPointerDown} onDragStart={event=>event.preventDefault()}>
  <div className="account-head"><div className="account-identity">{dragHandle}{brand}<div className="identity-copy">
   <div className="account-title">{data?.email??`Compte ${service}`}<span className={`pill ${error?'account-warning':'live'}`}>{error?'À vérifier':data?.unified?'Quota partagé':data?'Connecté':'Lecture en cours'}</span>{error&&<button type="button" className="account-reconnect" disabled={loading} onClick={onConnect}>Reconnecter</button>}</div>
   <p>{service} <span>·</span> {subscriptionPlan(data?.plan)}</p>
  </div></div><div className="account-actions">
   {time&&<span className="account-updated" title={`Dernière actualisation à ${time}`} aria-label={`Dernière actualisation à ${time}`}><Clock3 size={12}/>{time}</span>}
   <button type="button" className="icon-button" title="Actualiser" aria-label={`Actualiser ${service}`} disabled={loading} onClick={()=>void quota.refresh()}><RefreshCw size={15} className={loading?'spin':''}/></button>
   <button type="button" className="icon-button danger" title="Arrêter le suivi dans Nivra" aria-label={`Retirer ${service} de Nivra`} disabled={loading} onClick={quota.forget}><Trash2 size={15}/></button>
  </div></div>
  {data?<div className="quota-grid cursor-grok-windows">{windows?.length?windows.map(window=><div key={window.kind} className="quota-window"><div className="quota-label"><span>{window.kind==='shared'?'7 jours · réserve partagée':window.label}</span><span className="quota-number">{window.used===null?'—':<>{new Intl.NumberFormat('fr-FR',{maximumFractionDigits:1}).format(100-window.used)}<span>%</span></>}</span></div><div className="progress" role="progressbar" aria-label={window.label} aria-valuemin={0} aria-valuemax={100} aria-valuenow={window.used===null?undefined:100-window.used}><span style={{width:`${window.used===null?0:100-window.used}%`}}/></div><div className="quota-reset"><Clock3 size={12}/>{reset(window.reset)}</div></div>):<p className="form-note">Le service n’a communiqué aucun quota pour ce compte.</p>}</div>:<p className="form-note" role={error?'alert':'status'}><Info size={14}/>{loading?'Lecture du compte et de son quota…':error||'Actualise pour retrouver ton compte.'}</p>}
  {data&&error&&<p className="form-note" role="alert"><Info size={14}/>{error}</p>}
  {data?.warning&&<p className="form-note"><Info size={14}/>{data.warning}</p>}
  {data&&grok&&data.unified!==true&&<p className="form-note">Le service n’a pas confirmé de réserve commune pour ce compte.</p>}
 </article>;
 if(compact)return card;
 return <section className="cursor-grok-section" aria-label="Quotas Cursor et Grok">
  {data||quota.enabled?<div className="account-list">{card}</div>:<div className="section-header"><div><h2>Connecte ton compte</h2><p>Retrouve ton abonnement et les quotas communiqués par le service.</p></div><button type="button" className="button ghost" disabled={loading} onClick={onConnect}>Connecter mon compte</button></div>}
 </section>;
}
