import settingsIcon from './assets/settings-icon.png';
import { Check, Contrast, LayoutGrid, Moon, RotateCcw, Sun } from 'lucide-react';
import { UpdateSettings, type UpdaterState } from './Updater';

export type Appearance = { theme:'dark'|'light'; accent:'violet'|'blue'|'green'|'rose'; density:'comfortable'|'compact'; glow:boolean; glowIntensity:number; ribbonRevision:2 };
export const defaults:Appearance = {theme:'dark',accent:'violet',density:'comfortable',glow:true,glowIntensity:100,ribbonRevision:2};
export function readAppearance():Appearance {
  try {
    const value=JSON.parse(localStorage.getItem('nivra-appearance')||'{}');
    return {theme:value.theme==='light'?'light':'dark',accent:'violet',density:value.density==='compact'?'compact':'comfortable',glow:value.ribbonRevision===2&&typeof value.glow==='boolean'?value.glow:true,glowIntensity:value.ribbonRevision===2&&typeof value.glowIntensity==='number'&&Number.isFinite(value.glowIntensity)?Math.max(5,Math.min(100,value.glowIntensity)):100,ribbonRevision:2};
  } catch { return defaults; }
}
export function AppearanceSettings({value,onChange,updater}:{value:Appearance;onChange:(value:Appearance)=>void;updater:UpdaterState}) {
  return <div className="settings-content"><header className="settings-header"><div className="page-heading"><img className="overview-heading-icon" src={settingsIcon} alt=""/><div><h1>Réglages</h1><p>Adapte Nivra à tes préférences.</p></div></div><button className="button ghost" onClick={()=>onChange(defaults)}><RotateCcw size={14}/> Réinitialiser</button></header>
    <section className="settings-section"><div className="settings-heading"><div className="settings-icon"><Contrast size={17}/></div><div><h2>Apparence</h2><p>Choisis le thème de ton espace.</p></div></div>
      <div className="theme-options" role="group" aria-label="Thème"><button type="button" className={`theme-choice ${value.theme==='dark'?'chosen':''}`} aria-pressed={value.theme==='dark'} onClick={()=>onChange({...value,theme:'dark'})}><span className="theme-preview dark-preview"><span/><i/><i/></span><span className="choice-label"><Moon size={15}/> Sombre {value.theme==='dark'&&<Check size={15}/>}</span></button><button type="button" className={`theme-choice ${value.theme==='light'?'chosen':''}`} aria-pressed={value.theme==='light'} onClick={()=>onChange({...value,theme:'light'})}><span className="theme-preview light-preview"><span/><i/><i/></span><span className="choice-label"><Sun size={15}/> Clair {value.theme==='light'&&<Check size={15}/>}</span></button></div>
    </section>
    <section className="settings-section"><div className="settings-heading"><div className="settings-icon"><LayoutGrid size={17}/></div><div><h2>Espacement</h2><p>Choisis la densité d’affichage des comptes.</p></div></div><div className="density-options" role="group" aria-label="Espacement"><button type="button" aria-pressed={value.density==='comfortable'} className={value.density==='comfortable'?'chosen':''} onClick={()=>onChange({...value,density:'comfortable'})}>Confortable</button><button type="button" aria-pressed={value.density==='compact'} className={value.density==='compact'?'chosen':''} onClick={()=>onChange({...value,density:'compact'})}>Compact</button></div></section>
    <section className="settings-section"><div className="settings-heading"><div className="settings-icon"><span className="palette-icon"/></div><div><h2>Fond animé</h2><p>Ribbon Glow · Originkit</p></div><label className="glow-switch"><input type="checkbox" checked={value.glow} onChange={e=>onChange({...value,glow:e.target.checked})}/><span>Activer le fond animé</span></label></div><label className="glow-range">Intensité <span>{value.glowIntensity} %</span><input type="range" min="5" max="100" step="5" disabled={!value.glow} value={value.glowIntensity} onChange={e=>onChange({...value,glowIntensity:Number(e.target.value)})}/></label><p className="glow-help">Quand le fond est activé, il bouge et réagit à la souris, même si les animations Windows sont désactivées. Il se met en pause lorsque la fenêtre est masquée.</p></section>
    <UpdateSettings updater={updater}/>
    <p className="settings-note">Tes préférences sont enregistrées sur cet appareil.</p>
  </div>;
}
