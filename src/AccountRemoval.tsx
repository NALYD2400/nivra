import {useId,useRef} from 'react';
import {Trash2} from 'lucide-react';

const copy = {
 disconnect: {
  action:'Déconnecter',
  title:'Déconnecter ce compte ?',
  detail:'Tu devras reconnecter ce compte pour suivre ses quotas dans Nivra.',
 },
 remove: {
  action:'Retirer',
  title:'Retirer ce compte ?',
  detail:'Ce compte sera retiré de la vue d’ensemble.',
 },
 stop: {
  action:'Arrêter le suivi',
  title:'Arrêter le suivi de ce compte ?',
  detail:'Nivra cessera de relever les quotas Cursor et Grok.',
 },
};

export function AccountRemoval({account,kind,disabled,onConfirm}:{account:string;kind:keyof typeof copy;disabled?:boolean;onConfirm:()=>void}) {
 const dialog=useRef<HTMLDialogElement>(null);
 const titleId=useId();
 const detailId=useId();
 const {action,title,detail}=copy[kind];
 return <>
  <button type="button" className="icon-button danger" title={action} aria-label={`${action} · ${account}`} disabled={disabled} onClick={event=>{
   event.currentTarget.focus({preventScroll:true});
   dialog.current?.showModal();
  }}><Trash2 size={15}/></button>
  <dialog ref={dialog} className="reset-dialog account-removal-dialog" aria-labelledby={titleId} aria-describedby={detailId} onPointerDown={event=>event.stopPropagation()}>
   <h2 id={titleId}>{title}</h2>
   <div id={detailId}><p className="account-removal-name">{account}</p><p className="account-removal-detail">{detail}</p></div>
   <div className="account-removal-actions">
    <button type="button" className="button ghost" onClick={()=>dialog.current?.close()}>Annuler</button>
    <button type="button" className="button account-removal-confirm" disabled={disabled} onClick={()=>{
     dialog.current?.close();
     onConfirm();
    }}>{action}</button>
   </div>
  </dialog>
 </>;
}
