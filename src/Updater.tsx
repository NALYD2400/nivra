import { useCallback, useEffect, useRef, useState } from 'react';
import { getVersion } from '@tauri-apps/api/app';
import { isTauri } from '@tauri-apps/api/core';
import { relaunch } from '@tauri-apps/plugin-process';
import { check, type Update } from '@tauri-apps/plugin-updater';
import { Download, RefreshCw } from 'lucide-react';

type Phase = 'idle'|'checking'|'ready'|'current'|'downloading'|'error';
export function useNivraUpdater() {
  const [version,setVersion]=useState('');
  const [phase,setPhase]=useState<Phase>('idle');
  const [nextVersion,setNextVersion]=useState('');
  const [progress,setProgress]=useState<number|null>(null);
  const pending=useRef<Update|null>(null);
  const busy=useRef(false);
  const checkUpdates=useCallback(async (silent=false)=>{
    if(!isTauri()||busy.current)return;
    busy.current=true;
    if(!silent)setPhase('checking');
    try {
      const next=await check();
      if(pending.current) await pending.current.close();
      pending.current=next;
      setNextVersion(next?.version||'');
      setPhase(next?'ready':'current');
    } catch { if(!silent)setPhase('error'); }
    finally { busy.current=false; }
  },[]);
  useEffect(()=>{
    if(!isTauri())return;
    void getVersion().then(setVersion).catch(()=>{});
    void checkUpdates(true);
    const timer=window.setInterval(()=>void checkUpdates(true),6*60*60*1000);
    return()=>{window.clearInterval(timer);if(pending.current)void pending.current.close()};
  },[checkUpdates]);
  const install=useCallback(async ()=>{
    if(!pending.current||busy.current)return;
    busy.current=true;setProgress(null);setPhase('downloading');
    let downloaded=0,total=0;
    try {
      await pending.current.downloadAndInstall(event=>{
        if(event.event==='Started')total=event.data.contentLength||0;
        if(event.event==='Progress'){downloaded+=event.data.chunkLength;if(total)setProgress(Math.min(100,Math.round(downloaded/total*100)))}
      });
      await relaunch();
    } catch {setPhase('error')}
    finally {busy.current=false}
  },[]);
  return {version,phase,nextVersion,progress,checkUpdates,install};
}
export type UpdaterState = ReturnType<typeof useNivraUpdater>;

export function UpdateSettings({updater}:{updater:UpdaterState}) {
  const {version,phase,nextVersion,progress,checkUpdates,install}=updater;
  const status=phase==='ready'?`Version ${nextVersion} disponible.`:phase==='checking'?'Recherche en cours…':phase==='downloading'?`Téléchargement${progress===null?'…':` : ${progress} %`}`:phase==='current'?'Nivra est à jour.':phase==='error'?'Vérification impossible. Réessaie plus tard.':'Recherche automatique des nouvelles versions.';
  return <section className="settings-section update-settings"><div className="settings-heading"><div className="settings-icon"><Download size={17}/></div><div><h2>Mises à jour</h2><p>{version?`Version ${version}`:'Version de l’application'}</p></div></div><div className="update-row"><span role="status">{status}</span>{phase==='ready'?<button className="button primary" onClick={()=>void install()}>Installer et redémarrer</button>:<button className="button ghost" disabled={phase==='checking'||phase==='downloading'} onClick={()=>void checkUpdates()}><RefreshCw size={14} className={phase==='checking'?'spin':''}/> Rechercher</button>}</div></section>;
}
