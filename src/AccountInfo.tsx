import {useEffect, useRef, useState, type FormEvent} from 'react';
import {invoke, isTauri} from '@tauri-apps/api/core';
import {ExternalLink, Info, Pencil, X} from 'lucide-react';
import {subscriptionPlan} from './subscription-plan';
import type {Service} from './Services';
import './account-info.css';

type AccountDetails = {endDate: string; note: string};

const pages: Record<Service, string> = {
  Codex: 'https://chatgpt.com/settings/usage',
  'Claude Code': 'https://claude.ai/settings/usage',
  Cursor: 'https://cursor.com/dashboard',
  'xAI / Grok': 'https://grok.com/dashboard',
  Mistral: 'https://admin.mistral.ai/subscription',
};
const emptyDetails: AccountDetails = {endDate: '', note: ''};
const noteLimit = 1000;
const dayInMs = 86_400_000;
const dateFormatter = new Intl.DateTimeFormat('fr-FR', {dateStyle: 'long'});
const calendarFormatter = new Intl.DateTimeFormat('fr-FR', {dateStyle: 'long', timeZone: 'UTC'});

function calendarDate(value: string) {
  if (!/^[0-9]{4}-[0-9]{2}-[0-9]{2}$/.test(value) || value.startsWith('0000-')) return null;
  const date = new Date(`${value}T00:00:00Z`);
  return Number.isFinite(date.getTime()) && date.toISOString().slice(0, 10) === value ? date : null;
}

function readDetails(key: string | null): AccountDetails {
  if (key === null) return emptyDetails;
  try {
    const data = JSON.parse(localStorage.getItem(key) ?? 'null');
    return {
      endDate: typeof data?.endDate === 'string' && calendarDate(data.endDate) ? data.endDate : '',
      note: typeof data?.note === 'string' ? data.note.slice(0, noteLimit) : '',
    };
  } catch {
    return emptyDetails;
  }
}

function deadlineStatus(date: Date) {
  const today = new Date();
  const localDay = Date.UTC(today.getFullYear(), today.getMonth(), today.getDate());
  const days = (date.getTime() - localDay) / dayInMs;
  if (days < 0) return 'Échéance passée';
  if (days === 0) return 'Aujourd’hui';
  if (days === 1) return 'Demain';
  if (days <= 14) return `Dans ${days} jours`;
  return null;
}

export function AccountInfo({service, account, accountId, plan, billingCycleEnd}: {
  service: Service;
  account: string;
  accountId?: string;
  plan?: string | null;
  billingCycleEnd?: string | null;
}) {
  const storageKey = accountId ? `nivra-account-details-v1:${service}:${accountId}` : null;
  const dialog = useRef<HTMLDialogElement>(null);
  const editorButton = useRef<HTMLButtonElement>(null);
  const dateInput = useRef<HTMLInputElement>(null);
  const [details, setDetails] = useState(emptyDetails);
  const [draft, setDraft] = useState<AccountDetails | null>(null);
  const [saveError, setSaveError] = useState('');
  const [linkError, setLinkError] = useState(false);
  const endDate = calendarDate(details.endDate);
  const deadline = endDate ? deadlineStatus(endDate) : null;
  const billingDate = billingCycleEnd ? new Date(billingCycleEnd) : null;
  const cycle = billingDate && Number.isFinite(billingDate.getTime()) ? dateFormatter.format(billingDate) : null;
  const editing = draft !== null;

  useEffect(() => {
    if (editing) dateInput.current?.focus();
  }, [editing]);

  function openDetails() {
    setDetails(readDetails(storageKey));
    setDraft(null);
    setSaveError('');
    setLinkError(false);
    dialog.current?.showModal();
  }

  function finishEditing() {
    setDraft(null);
    setSaveError('');
    requestAnimationFrame(() => editorButton.current?.focus());
  }

  function saveDetails(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!storageKey || !draft) return;
    if (draft.endDate && !calendarDate(draft.endDate)) {
      setSaveError('Saisis une date de fin valide.');
      dateInput.current?.focus();
      return;
    }
    const next = {endDate: draft.endDate, note: draft.note.trim()};
    try {
      if (next.endDate || next.note) localStorage.setItem(storageKey, JSON.stringify(next));
      else localStorage.removeItem(storageKey);
    } catch {
      setSaveError('Impossible d’enregistrer les informations sur cet appareil.');
      return;
    }
    setDetails(next);
    finishEditing();
  }

  async function openService() {
    setLinkError(false);
    try {
      if (isTauri()) await invoke('open_usage', {service});
      else window.open(pages[service], '_blank', 'noopener,noreferrer');
    } catch {
      setLinkError(true);
    }
  }

  return <>
    <button type="button" className="icon-button" title="Informations sur le compte"
      aria-label={`Informations sur le compte ${account}`} onClick={openDetails}><Info size={16}/></button>
    <dialog ref={dialog} className="reset-dialog account-info-dialog"
      aria-label={`Informations sur le compte ${account}`}
      onPointerDown={event => {
        event.stopPropagation();
        if (event.target !== event.currentTarget) return;
        const {left, top, right, bottom} = event.currentTarget.getBoundingClientRect();
        if (event.clientX < left || event.clientX > right || event.clientY < top || event.clientY > bottom) {
          event.currentTarget.close();
        }
      }}>
      <header className="account-info-head">
        <div><h2>Informations du compte</h2><p>{account}</p></div>
        <button type="button" className="icon-button" aria-label="Fermer" onClick={() => dialog.current?.close()}><X size={18}/></button>
      </header>
      <dl className="account-info-list">
        <div><dt>Service</dt><dd>{service}</dd></div>
        <div><dt>Abonnement</dt><dd>{subscriptionPlan(plan)}</dd></div>
        {cycle && <div><dt>Fin du cycle de facturation</dt><dd>{cycle}</dd></div>}
        <div>
          <dt>Fin de l’abonnement</dt>
          <dd>{endDate ? <>
            <time dateTime={details.endDate}>{calendarFormatter.format(endDate)}</time>
            <span className="account-info-source">Saisie manuelle</span>
            {deadline && <span className="account-info-deadline">{deadline}</span>}
          </> : 'Non communiquée'}</dd>
        </div>
      </dl>
      {draft ? <form className="account-info-form" onSubmit={saveDetails}>
        <label>Fin de l’abonnement <span>(facultatif)</span>
          <input ref={dateInput} type="date" max="9999-12-31" value={draft.endDate}
            onChange={event => setDraft({...draft, endDate: event.target.value})}/>
        </label>
        <label>Note personnelle <span>(facultatif)</span>
          <textarea rows={3} maxLength={noteLimit} value={draft.note} placeholder="Ex. Vérifier le renouvellement"
            onChange={event => setDraft({...draft, note: event.target.value})}/>
        </label>
        <p className="account-info-note">Enregistré sur cet appareil pour ce compte.</p>
        {saveError && <p role="alert" className="account-info-note">{saveError}</p>}
        <div className="account-info-actions">
          <button type="button" className="button ghost" onClick={finishEditing}>Annuler</button>
          <button type="submit" className="button primary">Enregistrer</button>
        </div>
      </form> : <>
        {details.note && <section className="account-info-personal-note">
          <h3>Note personnelle</h3><p>{details.note}</p>
        </section>}
        <div className="account-info-actions account-info-actions-main">
          <button type="button" className="button ghost account-info-link" onClick={() => void openService()}>
            Ouvrir le service <ExternalLink size={14}/>
          </button>
          {storageKey && <button ref={editorButton} type="button" className="button primary" onClick={() => setDraft(details)}>
            <Pencil size={14}/>{details.endDate || details.note ? 'Modifier' : 'Ajouter une date ou une note'}
          </button>}
        </div>
        {linkError && <p role="alert" className="account-info-note">Impossible d’ouvrir le service.</p>}
      </>}
    </dialog>
  </>;
}
