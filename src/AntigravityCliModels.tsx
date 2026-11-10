import { useEffect, useState } from 'react';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { Copy, RefreshCw } from 'lucide-react';
import './gateway.css';
import {upsertAntigravityAccount, type GatewayAccount} from './antigravity-accounts';
export type {GatewayAccount} from './antigravity-accounts';

type CliModel={id:string;label:string};
type AccountModel=CliModel&{remaining:number|null;reset:string|null};
type QuotaWindow={group:string;window:string;remaining:number;reset:string|null};
export const accountsKey='nivra-antigravity-gateway-accounts-v1';
function resetLabel(value:string|null):string{if(!value)return 'Renouvellement non communiqué';const date=new Date(value);return Number.isNaN(date.getTime())?'Renouvellement non communiqué':`Renouvellement : ${date.toLocaleString('fr-FR')}`}
export function readAccounts():GatewayAccount[]{
 try{const value:unknown=JSON.parse(localStorage.getItem(accountsKey)||'[]');if(!Array.isArray(value))return [];return value.filter((item):item is GatewayAccount=>!!item&&typeof item==='object'&&typeof item.id==='string'&&typeof item.email==='string'&&typeof item.project==='string'&&typeof item.credential_ref==='string'&&/^[a-f0-9]{32}$/.test(item.credential_ref))}catch{return []}
}

export function AntigravityCliModels(){
 const [models,setModels]=useState<CliModel[]|null>(null);
 const [busy,setBusy]=useState(false);
 const [workspace,setWorkspace]=useState('');
 const [message,setMessage]=useState('');
 const [accounts,setAccounts]=useState<GatewayAccount[]>(readAccounts);
 useEffect(()=>{const refresh=()=>setAccounts(readAccounts());window.addEventListener('nivra-antigravity-accounts-changed',refresh);return()=>window.removeEventListener('nivra-antigravity-accounts-changed',refresh)},[]);
 const [accountModels,setAccountModels]=useState<{id:string;models:AccountModel[]|null;quota:QuotaWindow[]|null;modelsError:string|null;quotaError:string|null}|null>(null);
 const [tryModel,setTryModel]=useState('');
 const [prompt,setPrompt]=useState('');
 const [answer,setAnswer]=useState('');
 async function tryAccountModel(){
  const account=accounts.find(item=>item.id===accountModels?.id);
  if(!account||!tryModel||!prompt.trim())return;
  setBusy(true);setMessage('');setAnswer('');
  try{setAnswer(await invoke<string>('antigravity_try_model',{credentialRef:account.credential_ref,accountId:account.id,model:tryModel,prompt}))}catch(error){setMessage(String(error))}finally{setBusy(false)}
 }
 async function connectGateway(){
  if(!isTauri()){setMessage('Ouvre Nivra pour connecter un compte.');return}
  setBusy(true);setMessage('Termine la connexion Google dans le navigateur.');let account:GatewayAccount|null=null;
  try{
   account=await invoke<GatewayAccount>('antigravity_gateway_login');
   const {next,previous}=upsertAntigravityAccount(readAccounts(),account);localStorage.setItem(accountsKey,JSON.stringify(next));const current=account;account=null;setAccounts(next);setAccountModels(null);setTryModel('');setAnswer('');window.dispatchEvent(new Event('nivra-antigravity-accounts-changed'));
   let notice=previous?'Connexion du compte Google mise à jour. Si la passerelle utilisait ce compte, arrête-la puis réactive-le.':'Compte Google enregistré. Tu peux le sélectionner dans la page Passerelle.';
   if(current.project_error)notice+=` Projet Antigravity : ${current.project_error}`;
   if(previous&&previous.credential_ref!==current.credential_ref){try{await invoke('gateway_forget_key',{reference:previous.credential_ref})}catch{notice+=' L’ancienne connexion locale n’a pas pu être retirée du coffre Windows.'}}
   setMessage(notice);
  }catch(error){if(account){try{await invoke('gateway_forget_key',{reference:account.credential_ref})}catch{/* Le coffre Windows reste disponible pour retirer la référence. */}}setMessage(String(error))}finally{setBusy(false)}
 }
 async function forgetGateway(account:GatewayAccount){setBusy(true);setMessage('');try{await invoke('gateway_forget_key',{reference:account.credential_ref});const next=accounts.filter(item=>item.credential_ref!==account.credential_ref);localStorage.setItem(accountsKey,JSON.stringify(next));setAccounts(next);window.dispatchEvent(new Event('nivra-antigravity-accounts-changed'));setAccountModels(null);setMessage('Compte retiré de Nivra. Tu peux gérer les autorisations Google dans ton compte Google.')}catch(error){setMessage(String(error))}finally{setBusy(false)}}
 async function reconnectGateway(previous:GatewayAccount){
  if(!isTauri()){setMessage('Ouvre Nivra pour reconnecter ce compte.');return}
  setBusy(true);setMessage(`Reconnecte ${previous.email} dans le navigateur.`);let replacement:GatewayAccount|null=null;
  try{
   replacement=await invoke<GatewayAccount>('antigravity_gateway_login');
   if(replacement.id!==previous.id){await invoke('gateway_forget_key',{reference:replacement.credential_ref});replacement=null;setMessage('Ce n’est pas le même compte Google. Recommence avec le compte indiqué.');return}
   const next=accounts.map(item=>item.credential_ref===previous.credential_ref?replacement!:item);localStorage.setItem(accountsKey,JSON.stringify(next));setAccounts(next);setAccountModels(null);replacement=null;
   try{await invoke('gateway_forget_key',{reference:previous.credential_ref});setMessage('Compte reconnecté. Si la passerelle utilisait ce compte, arrête-la puis réactive-le.')}catch{setMessage('Compte reconnecté, mais l’ancienne autorisation locale n’a pas pu être retirée du coffre Windows.')}
  }catch(error){if(replacement){try{await invoke('gateway_forget_key',{reference:replacement.credential_ref})}catch{/* La référence reste dans le coffre Windows si son retrait échoue. */}}setMessage(String(error))}finally{setBusy(false)}
 }
 async function checkGateway(account:GatewayAccount){setBusy(true);setMessage('');setAccountModels(null);setTryModel('');setAnswer('');try{const status=await invoke<{email:string;project:string;models:AccountModel[]|null;quota:QuotaWindow[]|null;models_error:string|null;quota_error:string|null}>('antigravity_gateway_check',{credentialRef:account.credential_ref,accountId:account.id});const next=accounts.map(item=>item.credential_ref===account.credential_ref?{...item,email:status.email,project:status.project,project_error:null}:item);localStorage.setItem(accountsKey,JSON.stringify(next));setAccounts(next);setAccountModels({id:account.id,models:status.models,quota:status.quota,modelsError:status.models_error,quotaError:status.quota_error});setMessage(`Compte Google vérifié pour ${status.email}.${status.models_error||status.quota_error?' Certaines données restent indisponibles.':' Tu peux sélectionner ce compte dans la page Passerelle.'}`)}catch(error){setMessage(String(error))}finally{setBusy(false)}}
 async function refresh(){
  if(!isTauri()){setMessage('Ouvre Nivra pour consulter Antigravity CLI.');return}
  setBusy(true);setMessage('');
  try{setModels(await invoke<CliModel[]>('antigravity_cli_models'))}catch(error){setModels(null);setMessage(String(error))}finally{setBusy(false)}
 }
 async function copy(id:string){try{await navigator.clipboard.writeText(id);setMessage('Identifiant copié.')}catch{setMessage('Sélectionne l’identifiant pour le copier.')}}
 async function login(){
  if(!isTauri()){setMessage('Ouvre Nivra pour connecter Antigravity CLI.');return}
  setBusy(true);setMessage('');setModels(null);
  try{await invoke('antigravity_cli_login');setMessage('Termine la connexion dans la fenêtre Antigravity CLI, puis recharge les modèles ici.')}catch(error){setMessage(String(error))}finally{setBusy(false)}
 }
 async function launch(model:string){
  if(!isTauri()){setMessage('Ouvre Nivra pour lancer Antigravity CLI.');return}
  setBusy(true);setMessage('');
  try{await invoke('antigravity_cli_launch',{model,workspace});setMessage('Ouverture d’Antigravity CLI demandée. Vérifie la connexion et le modèle dans la nouvelle fenêtre.')}catch(error){setMessage(String(error))}finally{setBusy(false)}
 }
 return <section className="gateway-panel antigravity-cli-panel" aria-labelledby="antigravity-cli-title">
  <h2 id="antigravity-cli-title">Modèles d’Antigravity CLI</h2>
  <p>Comptes Google pour la passerelle Antigravity : connexion expérimentale. Après ajout, sélectionne le compte sur la page Passerelle pour utiliser les modèles disponibles.</p>
  <button type="button" className="button ghost" onClick={()=>void connectGateway()} disabled={busy}>Ajouter un compte Google</button>
  {accounts.length>0&&<ul className="gateway-models">{accounts.map(account=><li key={account.credential_ref}><span>{account.email}{account.project_error&&<small> · Projet indisponible : {account.project_error}</small>}</span><button type="button" className="button ghost" disabled={busy} onClick={()=>void checkGateway(account)}>Vérifier</button><button type="button" className="button ghost" disabled={busy} onClick={()=>void reconnectGateway(account)}>Reconnecter</button><button type="button" className="button ghost" disabled={busy} onClick={()=>void forgetGateway(account)}>Retirer</button></li>)}</ul>}
  {accountModels&&<div><p>Quotas pour {accounts.find(account=>account.id===accountModels.id)?.email} :</p>{accountModels.quota?.length?<ul className="gateway-models">{accountModels.quota.map((quota,index)=><li key={`${quota.group}-${quota.window}-${index}`}><span>{quota.group} · {quota.window==='5h'?'5 heures':'semaine'}</span><strong>{Math.round(quota.remaining*100)} % restants</strong><span>{resetLabel(quota.reset)}</span></li>)}</ul>:<p>{accountModels.quotaError?`Lecture des quotas impossible : ${accountModels.quotaError}`:'Google n’a communiqué aucun quota pour ce compte.'}</p>}<p>Modèles renvoyés pour ce compte. Les pourcentages par modèle sont les valeurs communiquées par Google : ils peuvent indiquer la disponibilité et ne représentent pas nécessairement un quota complet. La présence d’un modèle ne garantit pas son utilisation.</p>{accountModels.models===null?<p>{accountModels.modelsError?`Lecture des modèles impossible : ${accountModels.modelsError}`:'Modèles indisponibles pour ce compte.'}</p>:accountModels.models.length?<ul className="gateway-models">{accountModels.models.map(model=><li key={model.id}><span>{model.label}</span><code>{model.id}</code>{model.remaining!==null&&<span>Valeur indiquée : {Math.round(model.remaining*100)} % · {resetLabel(model.reset)}</span>}</li>)}</ul>:<p>Aucun modèle renvoyé.</p>}</div>}
  {accountModels?.models?.length? <div className="gateway-form"><h3>Essayer un modèle</h3><p>Envoie une demande texte unique au compte sélectionné ; elle utilise son quota. Ce test expérimental ne passe pas encore par la passerelle.</p><label>Modèle<select value={tryModel} disabled={busy} onChange={e=>setTryModel(e.target.value)}><option value="">Choisir un modèle</option>{accountModels.models.map(model=><option value={model.id} key={model.id}>{model.label}</option>)}</select></label><label>Demande<textarea value={prompt} maxLength={32000} disabled={busy} onChange={e=>setPrompt(e.target.value)}/></label><button type="button" className="button ghost" disabled={busy||!tryModel||!prompt.trim()} onClick={()=>void tryAccountModel()}>Envoyer</button>{answer&&<div role="status"><strong>Réponse</strong><p style={{whiteSpace:'pre-wrap'}}>{answer}</p></div>}</div>:null}
  <p>Consulte les modèles renvoyés par le CLI installé sur cet appareil. Leur présence ne garantit pas l’accès effectif à chaque modèle.</p>
  <p>La session utilise le compte connecté dans Antigravity CLI, qui peut être différent des profils affichés dans Nivra.</p>
  <button type="button" className="button ghost" onClick={()=>void login()} disabled={busy}>Connecter le compte du CLI</button>
  <label className="gateway-provider-select">Dossier du projet (Documents par défaut)<input value={workspace} onChange={e=>setWorkspace(e.target.value)} placeholder="C:\\…\\mon-projet" disabled={busy}/></label>
  <button type="button" className="button ghost" onClick={()=>void refresh()} disabled={busy}><RefreshCw size={15} className={busy?'spin':''}/>{busy?'Lecture…':models?'Actualiser les modèles':'Charger les modèles'}</button>
  {models&&<ul className="gateway-models">{models.map(model=><li key={model.id}><div className="antigravity-cli-model"><strong>{model.label}</strong><code>{model.id}</code></div><button type="button" className="button ghost" disabled={busy} onClick={()=>void launch(model.id)}>Ouvrir</button><button type="button" className="icon-button" aria-label={`Copier ${model.id}`} onClick={()=>void copy(model.id)}><Copy size={15}/></button></li>)}</ul>}
  {message&&<p className="gateway-message" role="status">{message}</p>}
  <p>La session du CLI et les comptes ajoutés à la passerelle sont indépendants.</p>
 </section>;
}
