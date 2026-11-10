import { useEffect, useRef, type ReactNode } from 'react';
import { ArrowUpRight, Check, CircleAlert, X } from 'lucide-react';
import './account-connection.css';
import { ConnectionIcon } from './ConnectionIcons';

type Service = 'Codex' | 'Claude Code' | 'Cursor' | 'xAI / Grok';
type Props = {
 service:Service; busy:boolean; error:string;
 onService:(service:Service)=>void; onClose:()=>void; onConnect:()=>void;
 renderBrand:(service:Service)=>ReactNode;
};
const services:Service[]=['Codex','Claude Code','Cursor','xAI / Grok'];
const content = {
 Cursor:{name:'Cursor',title:'Connecte ton compte Cursor',description:'Connecte-toi sur la page officielle dans la fenêtre qui va s’ouvrir. Nivra retrouve ensuite ton compte et lit tes quotas automatiquement.',features:[],action:'Connecter mon compte',waiting:'Termine la connexion dans la fenêtre officielle. Ton quota apparaîtra ensuite ici.',note:'Ta session est conservée dans le profil web de Nivra par Windows. Aucun modèle n’est lancé.'},
 'xAI / Grok':{name:'xAI / Grok',title:'Connecte ton compte unifié',description:'Connecte ton compte unifié sur la page officielle. Nivra lit ensuite la réserve partagée entre Cursor, Grok et Grok Bot.',features:[],action:'Connecter mon compte',waiting:'Termine la connexion dans la fenêtre officielle. Ta réserve apparaîtra ensuite ici.',note:'Seule la réserve de ton abonnement est suivie, sans consommer de crédits.'},
 Codex:{name:'ChatGPT / Codex',title:'Retrouve tes quotas ChatGPT',description:'Connecte ton compte dans le navigateur. Nivra récupère ensuite ton adresse et les quotas disponibles.',features:['Quotas et renouvellements','Plusieurs comptes, suivis séparément'],action:'Continuer avec ChatGPT',waiting:'Termine la connexion dans ton navigateur.',note:'Si le coffre Windows refuse le jeton, Codex le conserve dans un fichier local non chiffré de ton profil Windows.'},
 'Claude Code':{name:'Claude Code',title:'Ajoute ton compte Claude',description:'La connexion s’ouvre dans le navigateur avec Claude Code. Ton compte sera ajouté à Nivra une fois connecté.',features:['Adresse du compte, si disponible','Profil distinct pour chaque compte'],action:'Continuer avec Claude',waiting:'Termine la connexion dans ton navigateur.',note:'Les identifiants de ce profil sont conservés localement par Claude Code.'},
};

export function AccountConnection({service,busy,error,onService,onClose,onConnect,renderBrand}:Props){
 const dialog=useRef<HTMLDivElement>(null);
 const close=useRef<HTMLButtonElement>(null);
 const info=content[service];
 useEffect(()=>{
  const previous=document.activeElement instanceof HTMLElement?document.activeElement:null;
  close.current?.focus();
  return()=>{if(previous?.isConnected)previous.focus()};
 },[]);
 return <div className="overlay connection-overlay" onMouseDown={event=>{if(event.target===event.currentTarget)onClose()}}>
  <div ref={dialog} className="connection-dialog" role="dialog" aria-modal="true" aria-labelledby="connection-title" aria-describedby="connection-subtitle" onKeyDown={event=>{
   if(event.key!=='Tab')return;
   const items=Array.from(dialog.current?.querySelectorAll<HTMLElement>('button:not(:disabled), a[href], summary')??[]);
   const first=items[0],last=items[items.length-1];
   if(event.shiftKey&&document.activeElement===first){event.preventDefault();last?.focus()}
   else if(!event.shiftKey&&document.activeElement===last){event.preventDefault();first?.focus()}
  }}>
   <header className="connection-header"><div><h2 id="connection-title">Ajouter un compte</h2><p id="connection-subtitle">Choisis le service à retrouver dans Nivra.</p></div><button ref={close} type="button" className="icon-button" aria-label="Fermer la connexion" onClick={onClose}><X size={19}/></button></header>
   <div className="connection-services" role="group" aria-label="Service à connecter">{services.map(item=><button key={item} type="button" aria-pressed={service===item} disabled={busy} onClick={()=>onService(item)}>{renderBrand(item)}<span>{content[item].name}</span>{service===item&&<Check size={15} className="connection-check"/>}</button>)}</div>
   <section key={service} className="connection-body" aria-label={info.name} aria-busy={busy}>
    <h3>{info.title}</h3>{!busy&&<p className="connection-description">{info.description}</p>}
    {service==='Claude Code'&&<p className="connection-availability"><ConnectionIcon kind="info" size={16}/> Les quotas de ce service ne sont pas encore disponibles.</p>}
    {!busy&&info.note&&<p className="connection-storage">{info.note}</p>}
    {busy&&<div className="connection-progress" role="status"><ConnectionIcon kind="loading" size={20} className="spin"/><div><strong>Connexion en cours</strong><p>{info.waiting}</p></div><ConnectionIcon kind="browser" size={20}/></div>}
    {error&&<div className="connection-error" role="alert"><CircleAlert size={17}/><div><strong>La connexion n’a pas abouti</strong><p>{error}</p></div></div>}
   </section>
   <footer className="connection-footer"><button type="button" className="button connection-cancel" onClick={onClose}>{busy?'Annuler la connexion':'Fermer'}</button><button type="button" className="button primary connection-submit" disabled={busy} onClick={onConnect}>{busy?<><ConnectionIcon kind="loading" size={16} className="spin"/> En attente…</>:<>{error?'Réessayer':info.action}<ArrowUpRight size={16}/></>}</button></footer>
  </div>
 </div>;
}
