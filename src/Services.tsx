import type {ReactNode} from 'react';
import {ArrowRight,Link2,Plus} from 'lucide-react';
import servicesIcon from './assets/summary-services.png';
import './services.css';

type Service='Codex'|'Claude Code'|'Cursor'|'xAI / Grok';
export type ServiceCard={
 service:Service;name:string;description:string;accounts:number;identity:string;
 plans:string[];status:'available'|'connected'|'loading'|'attention';
 shared:boolean;quota:{remaining:number;label:string;account?:string}|null;
};
type Props={cards:ServiceCard[];onOpen:(service:Service)=>void;onAdd:()=>void;renderBrand:(service:Service)=>ReactNode};

export function Services({cards,onOpen,onAdd,renderBrand}:Props){
 const connected=cards.filter(card=>card.accounts>0);
 const shared=connected.some(card=>card.shared);
 return <div className="services-page">
  <header className="page-head services-head">
   <div className="page-heading"><span className="services-heading-art"><img src={servicesIcon} alt=""/></span><div><h1>Services</h1><p>Tes connexions, réunies au même endroit.</p></div></div>
   <button type="button" className="button primary" onClick={onAdd}><Plus size={16}/>Ajouter un compte</button>
  </header>
  {connected.length===0?<div className="empty services-empty"><span className="services-empty-icon"><Plus size={22}/></span><div><b>Aucun service connecté</b><p>Ajoute un compte pour retrouver son service et ses quotas ici.</p><button type="button" className="text-button" onClick={onAdd}>Connecter un compte <ArrowRight size={14}/></button></div></div>:<div className="services-grid">{connected.map(card=><button type="button" key={card.service} className={`service-tile${card.shared?' is-shared':''}`} onClick={()=>onOpen(card.service)} aria-label={`Gérer ${card.name}`}>
   <span className="service-tile-top"><span className="service-tile-brand">{renderBrand(card.service)}</span><span className={`service-state ${card.status}`}><i/>{card.status==='loading'?'Actualisation':card.status==='attention'?'À vérifier':card.status==='connected'?'Connecté':'À connecter'}</span></span>
   <span className="service-tile-heading"><strong>{card.name}</strong><span>{card.description}</span></span>
   <span className="service-tile-account"><span title={card.identity}>{card.accounts?card.identity:card.status==='loading'?'Lecture du compte…':'Aucun compte ajouté'}</span><span className="service-tile-plan">{card.plans.length?`${card.plans.slice(0,2).join(' · ')}${card.plans.length>2?` +${card.plans.length-2}`:''}`:card.accounts?'Offre non communiquée':'Connecte ton compte pour commencer'}</span></span>
   <span className="service-tile-usage">{card.quota?<><span className="service-tile-quota-label"><span title={card.quota.account}>{card.quota.label}</span><strong>{new Intl.NumberFormat('fr-FR',{maximumFractionDigits:1}).format(card.quota.remaining)}<small> % restants</small></strong></span><span className="service-tile-track" aria-hidden="true"><span style={{width:`${card.quota.remaining}%`}}/></span></>:<span className="service-tile-no-quota">{card.service==='Claude Code'?'Quotas non disponibles pour le moment':card.accounts?'Quota momentanément indisponible':'Tes quotas apparaîtront ici'}</span>}</span>
   <span className="service-tile-footer"><span>{card.shared?<><Link2 size={13}/>Réserve partagée</>:card.accounts?`${card.accounts} compte${card.accounts>1?'s':''} suivi${card.accounts>1?'s':''}`:'Connexion disponible'}</span><span className="service-tile-action">{card.accounts?'Voir mes comptes':'Découvrir'}<ArrowRight size={15}/></span></span>
  </button>)}</div>}
  {shared&&<p className="services-shared-note"><Link2 size={14}/>Cursor et xAI / Grok utilisent la même réserve pour ton compte unifié.</p>}
 </div>;
}
