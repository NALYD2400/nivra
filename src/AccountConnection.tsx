import { useEffect, useRef, type ReactNode } from 'react';
import { ArrowUpRight, Check, CircleAlert, X } from 'lucide-react';
import './account-connection.css';
import { ConnectionIcon } from './ConnectionIcons';

type Service = 'Codex' | 'Claude Code' | 'Cursor' | 'Antigravity';
type Props = {
 service:Service; busy:boolean; error:string;
 onService:(service:Service)=>void; onClose:()=>void; onConnect:()=>void;
 renderBrand:(service:Service)=>ReactNode;
};
const services:Service[]=['Codex','Claude Code','Cursor','Antigravity'];
const content = {
 Codex:{name:'ChatGPT / Codex',title:'Retrouve tes quotas ChatGPT',description:'Connecte ton compte dans le navigateur. Nivra récupère ensuite ton adresse et les quotas disponibles.',features:['Quotas et renouvellements','Plusieurs comptes, suivis séparément'],action:'Continuer avec ChatGPT',waiting:'Termine la connexion dans ton navigateur.',note:'Si le coffre Windows refuse le jeton, Codex le conserve dans un fichier local non chiffré de ton profil Windows.'},
 'Claude Code':{name:'Claude Code',title:'Ajoute ton compte Claude',description:'La connexion s’ouvre dans le navigateur avec Claude Code. Ton compte sera ajouté à Nivra une fois connecté.',features:['Adresse du compte, si disponible','Profil distinct pour chaque compte'],action:'Continuer avec Claude',waiting:'Termine la connexion dans ton navigateur.',note:'Les identifiants de ce profil sont conservés localement par Claude Code.'},
 Cursor:{name:'Cursor',title:'Ouvre un profil Cursor séparé',description:'Une nouvelle fenêtre Cursor s’ouvre. Connecte-toi dans cette fenêtre pour ajouter ton compte à Nivra.',features:['Un profil Cursor indépendant','Possibilité d’ajouter plusieurs comptes'],action:'Ouvrir Cursor',waiting:'Termine la connexion dans la fenêtre Cursor.',note:''},
 Antigravity:{name:'Antigravity',title:'Connecte ton compte Google',description:'La page de connexion Google s’ouvre dans ton navigateur pour ajouter ton compte à Nivra et à la passerelle.',features:['Compte et modèles Antigravity','Quotas disponibles selon le compte'],action:'Continuer avec Google',waiting:'Termine la connexion dans ton navigateur.',note:'La connexion est expérimentale. Le jeton est conservé dans le coffre Windows.'},
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
    {(service==='Claude Code'||service==='Cursor')&&<p className="connection-availability"><ConnectionIcon kind="info" size={16}/> Les quotas de ce service ne sont pas encore disponibles.</p>}
    {service==='Antigravity'&&<p className="connection-availability"><ConnectionIcon kind="info" size={16}/> Après la connexion, utilise « Vérifier » sur la page Antigravity pour consulter les modèles et quotas disponibles.</p>}
    {!busy&&info.note&&<p className="connection-storage">{info.note}</p>}
    {busy&&<div className="connection-progress" role="status"><ConnectionIcon kind="loading" size={20} className="spin"/><div><strong>Connexion en cours</strong><p>{info.waiting}</p></div><ConnectionIcon kind="browser" size={20}/></div>}
    {error&&<div className="connection-error" role="alert"><CircleAlert size={17}/><div><strong>La connexion n’a pas abouti</strong><p>{error}</p></div></div>}
   </section>
   <footer className="connection-footer"><button type="button" className="button connection-cancel" onClick={onClose}>{busy?'Annuler la connexion':'Fermer'}</button><button type="button" className="button primary connection-submit" disabled={busy} onClick={onConnect}>{busy?<><ConnectionIcon kind="loading" size={16} className="spin"/> En attente…</>:<>{error?'Réessayer':info.action}<ArrowUpRight size={16}/></>}</button></footer>
  </div>
 </div>;
}
