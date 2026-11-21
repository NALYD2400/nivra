import type { ReactNode } from 'react';
import { Check, Moon, RotateCcw, Sun } from 'lucide-react';
import { UpdateSettings, type UpdaterState } from './Updater';
import settingsIcon from './assets/settings-icon.png';
import themeIcon from './assets/settings-appearance.png';
import densityIcon from './assets/settings-spacing.png';
import animationsIcon from './assets/settings-animations.png';
import glowIcon from './assets/settings-background.png';
import updatesIcon from './assets/settings-updates.png';
import './appearance.css';

export type Appearance = {
  theme: 'dark' | 'light';
  accent: 'violet' | 'blue' | 'green' | 'rose';
  density: 'comfortable' | 'compact';
  animations: boolean;
  glow: boolean;
  glowIntensity: number;
  ribbonRevision: 2;
};

export const defaults: Appearance = {
  theme: 'dark',
  accent: 'violet',
  density: 'comfortable',
  animations: true,
  glow: true,
  glowIntensity: 100,
  ribbonRevision: 2,
};

export function readAppearance(): Appearance {
  try {
    const value = JSON.parse(localStorage.getItem('nivra-appearance') || '{}');
    const hasRibbon = value.ribbonRevision === defaults.ribbonRevision;
    return {
      ...defaults,
      theme: value.theme === 'light' ? 'light' : 'dark',
      density: value.density === 'compact' ? 'compact' : 'comfortable',
      animations: typeof value.animations === 'boolean' ? value.animations : defaults.animations,
      glow: hasRibbon && typeof value.glow === 'boolean' ? value.glow : defaults.glow,
      glowIntensity: hasRibbon && Number.isFinite(value.glowIntensity)
        ? Math.max(5, Math.min(100, value.glowIntensity))
        : defaults.glowIntensity,
    };
  } catch {
    return defaults;
  }
}

function PreferenceHeading({ id, title, detail, icon }: {
  id: string;
  title: string;
  detail: string;
  icon: string;
}) {
  return <div className="preference-heading">
    <span className="preference-icon"><img src={icon} alt="" width={40} height={40}/></span>
    <div>
      <h2 id={id}>{title}</h2>
      <p id={`${id}-detail`}>{detail}</p>
    </div>
  </div>;
}

function PreferenceSwitch({ heading, checked, onChange }: {
  heading: string;
  checked: boolean;
  onChange: (checked: boolean) => void;
}) {
  return <label className="preference-switch">
    <input type="checkbox" role="switch" checked={checked} aria-labelledby={heading}
      aria-describedby={`${heading}-detail`} onChange={event => onChange(event.target.checked)}/>
    <span aria-hidden="true"/>
  </label>;
}

function PreferenceRow({ children, control, ...heading }: Parameters<typeof PreferenceHeading>[0] & {
  children?: ReactNode;
  control?: ReactNode;
}) {
  return <section className="preference-row" aria-labelledby={heading.id}>
    <div className="preference-row-layout">
      <PreferenceHeading {...heading}/>
      {control}
    </div>
    {children}
  </section>;
}

const themes = [
  { theme: 'dark', label: 'Sombre', Icon: Moon },
  { theme: 'light', label: 'Clair', Icon: Sun },
] as const;

const densities = [
  { density: 'comfortable', label: 'Confortable' },
  { density: 'compact', label: 'Compact' },
] as const;

export function AppearanceSettings({ value, onChange, updater }: {
  value: Appearance;
  onChange: (value: Appearance) => void;
  updater: UpdaterState;
}) {
  return <div className="preferences">
    <header className="preferences-header">
      <div className="page-heading">
        <img className="overview-heading-icon" src={settingsIcon} alt=""/>
        <h1>Réglages</h1>
      </div>
      <button type="button" className="button ghost" onClick={() => onChange(defaults)}>
        <RotateCcw size={14}/> Réinitialiser
      </button>
    </header>
    <div className="preferences-panel">
      <PreferenceRow id="settings-appearance" title="Apparence" detail="Choisis le thème de l’interface." icon={themeIcon}>
        <div className="preference-themes" role="group" aria-label="Thème">
          {themes.map(({ theme, label, Icon }) => <button key={theme} type="button" className="preference-theme"
            aria-pressed={value.theme === theme} onClick={() => onChange({ ...value, theme })}>
            <span className={`preference-theme-preview preview-${theme}`} aria-hidden="true">
              <span className="preview-nav"><i/><i/><i/></span>
              <span className="preview-content"><i/><i/><i/></span>
            </span>
            <span className="preference-theme-label">
              <Icon size={15} aria-hidden="true"/> {label}
              {value.theme === theme && <Check className="preference-theme-check" size={16} aria-hidden="true"/>}
            </span>
          </button>)}
        </div>
      </PreferenceRow>
      <PreferenceRow id="settings-spacing" title="Espacement" detail="Règle la densité des cartes de compte." icon={densityIcon}
        control={<div className="preference-density" role="group" aria-labelledby="settings-spacing">
          {densities.map(({ density, label }) => <button key={density} type="button" aria-pressed={value.density === density}
            onClick={() => onChange({ ...value, density })}>{label}</button>)}
        </div>}/>
    </div>
    <div className="preferences-panel">
      <PreferenceRow id="settings-animations" title="Animations de l’interface" detail="Anime les changements de page et de thème." icon={animationsIcon}
        control={<PreferenceSwitch heading="settings-animations" checked={value.animations}
          onChange={animations => onChange({ ...value, animations })}/>}/>
      <PreferenceRow id="settings-background" title="Fond animé" detail="Le ruban réagit à la souris et se met en pause quand la fenêtre est masquée." icon={glowIcon}
        control={<PreferenceSwitch heading="settings-background" checked={value.glow}
          onChange={glow => onChange({ ...value, glow })}/>}>
        <div className="preference-range">
          <label htmlFor="settings-glow-intensity">Intensité du fond</label>
          <output htmlFor="settings-glow-intensity">{value.glowIntensity} %</output>
          <input id="settings-glow-intensity" type="range" min="5" max="100" step="5" disabled={!value.glow}
            value={value.glowIntensity} aria-valuetext={`${value.glowIntensity} %`}
            onChange={event => onChange({ ...value, glowIntensity: event.target.valueAsNumber })}/>
        </div>
      </PreferenceRow>
    </div>
    <div id="updates" className="preferences-panel">
      <PreferenceRow id="settings-updates" title="Mises à jour"
        detail={updater.version ? `Version ${updater.version}` : 'Version de l’application'} icon={updatesIcon}
        control={<UpdateSettings updater={updater}/>}/>
    </div>
  </div>;
}
