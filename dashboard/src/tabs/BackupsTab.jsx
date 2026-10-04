import { useCallback, useEffect, useState } from 'react';
import { ArchiveRestore, Download, Trash2, Lock, KeyRound, AlertTriangle, Smartphone } from 'lucide-react';
import { gql, getToken, formatBytes } from '../api.js';

/**
 * The account's cloud backups, as far as this server can see them.
 *
 * Which is the label and nothing else. A backup is sealed on the phone under a key derived from
 * the account's vault key, which never reaches the server or this page, so everything shown here
 * is what the device wrote on the envelope: when, from where, how big, and how many records of
 * each kind. Restoring happens in the app, which holds the key.
 */
const LIST_QUERY = `query Backups {
  vaultBackups {
    id createdAt deviceId deviceName appVersion plainBytes sealedBytes includesAccounts sha256
    sections { name count }
  }
  vaultLimits { keepPerAccount maxSealedBytes }
}`;

const DELETE_MUTATION = `mutation DeleteBackup($id: String!) { deleteVaultBackup(id: $id) }`;

/** What each section the app sends is called here. An unknown one is shown by its own name. */
const SECTION_NAMES = {
  SETTINGS: 'Settings',
  ACCOUNTS: 'Sign-ins',
  HISTORY: 'Listening history',
  LIBRARY: 'Library & playlists',
  MERGES: 'Track merges',
  EPISODES: 'Podcast progress'
};

function sectionName(name) {
  return SECTION_NAMES[name] ?? name.charAt(0) + name.slice(1).toLowerCase();
}

function ago(iso) {
  const seconds = Math.max(0, (Date.now() - new Date(iso).getTime()) / 1000);
  if (seconds < 90) return 'just now';
  const units = [['day', 86400], ['hour', 3600], ['minute', 60]];
  for (const [unit, size] of units) {
    const n = Math.floor(seconds / size);
    if (n >= 1) return `${n} ${unit}${n === 1 ? '' : 's'} ago`;
  }
  return 'just now';
}

export default function BackupsTab({ onUnauthorized }) {
  const [backups, setBackups] = useState(null);
  const [limits, setLimits] = useState(null);
  const [notice, setNotice] = useState('');
  const [busy, setBusy] = useState(null);

  const load = useCallback(async () => {
    try {
      const res = await gql(LIST_QUERY);
      const body = await res.json();
      const error = body?.errors?.[0];
      if (error) {
        setNotice(
          error.extensions?.code === 'FEATURE_DISABLED'
            ? 'Cloud backups are switched off on this server.'
            : error.message
        );
        setBackups([]);
        return;
      }
      setBackups(body?.data?.vaultBackups ?? []);
      setLimits(body?.data?.vaultLimits ?? null);
    } catch (error) {
      if (error.unauthorized) onUnauthorized?.();
    }
  }, [onUnauthorized]);

  useEffect(() => {
    load();
    // A backup lands at most a few times a day; a minute is plenty to see one arrive.
    const timer = setInterval(load, 60000);
    return () => clearInterval(timer);
  }, [load]);

  async function download(backup) {
    setBusy(backup.id);
    setNotice('');
    try {
      const res = await fetch(`/api/v1/vault/backups/${encodeURIComponent(backup.id)}`, {
        headers: { Authorization: `Bearer ${getToken()}` }
      });
      if (res.status === 401) return onUnauthorized?.();
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const url = URL.createObjectURL(await res.blob());
      const link = document.createElement('a');
      link.href = url;
      link.download = `wanda-${backup.createdAt.slice(0, 10)}.wandavault`;
      link.click();
      URL.revokeObjectURL(url);
    } catch {
      setNotice('Could not download that backup.');
    } finally {
      setBusy(null);
    }
  }

  async function remove(backup) {
    if (!window.confirm('Delete this backup? It cannot be recovered.')) return;
    setBusy(backup.id);
    setNotice('');
    try {
      const res = await gql(DELETE_MUTATION, { id: backup.id });
      const body = await res.json();
      if (body?.errors?.length) setNotice(body.errors[0].message);
      else setBackups(current => current.filter(item => item.id !== backup.id));
    } catch (error) {
      if (error.unauthorized) onUnauthorized?.();
      else setNotice('Could not reach the server.');
    } finally {
      setBusy(null);
    }
  }

  const [latest, ...older] = backups ?? [];

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: '16px' }}>
      {notice && (
        <div className="card" style={{ display: 'flex', gap: '10px', alignItems: 'flex-start' }}>
          <AlertTriangle size={16} style={{ flexShrink: 0, marginTop: '2px' }} />
          <div style={{ fontSize: '13px' }}>{notice}</div>
        </div>
      )}

      <div className="card vault-explainer">
        <Lock size={18} />
        <div>
          Sealed on your phone with your vault key before it is sent. This server stores it and cannot
          open it — what you see here is only the label the app wrote on it.
          {limits && (
            <span className="row-sub">
              {' '}Your last {limits.keepPerAccount} backups are kept, up to {formatBytes(limits.maxSealedBytes)} each.
            </span>
          )}
        </div>
      </div>

      {backups && !latest && (
        <div className="card empty-hint">
          No backups yet. In Wanda, open <strong>Settings → Backup</strong> and turn on
          <strong> Back up to Agro</strong>.
        </div>
      )}

      {latest && (
        <BackupCard backup={latest} latest busy={busy === latest.id} onDownload={download} onDelete={remove} />
      )}

      {older.length > 0 && (
        <div className="card">
          <div className="card-header"><div className="card-title">Earlier backups</div></div>
          <div style={{ display: 'flex', flexDirection: 'column', gap: '12px' }}>
            {older.map(b => (
              <BackupCard key={b.id} backup={b} busy={busy === b.id} onDownload={download} onDelete={remove} />
            ))}
          </div>
        </div>
      )}
    </div>
  );
}

function BackupCard({ backup, latest = false, busy, onDownload, onDelete }) {
  const saved = backup.plainBytes > 0 ? Math.round((1 - backup.sealedBytes / backup.plainBytes) * 100) : 0;
  return (
    <div className={latest ? 'card vault-card vault-latest' : 'vault-card'}>
      <div className="vault-head">
        <ArchiveRestore size={latest ? 22 : 16} />
        <div style={{ flex: 1, minWidth: 0 }}>
          <div className={latest ? 'vault-when' : 'card-title'}>
            {latest ? 'Latest backup · ' : ''}{ago(backup.createdAt)}
          </div>
          <div className="row-sub">
            {new Date(backup.createdAt).toLocaleString()}
            {' · '}<Smartphone size={11} /> {backup.deviceName || backup.deviceId}
            {backup.appVersion ? ` · Wanda ${backup.appVersion}` : ''}
          </div>
        </div>
        <button className="btn btn-secondary" disabled={busy} onClick={() => onDownload(backup)} title="Download the sealed file">
          <Download size={14} />
        </button>
        <button className="btn btn-danger" disabled={busy} onClick={() => onDelete(backup)} title="Delete">
          <Trash2 size={14} />
        </button>
      </div>

      <div className="vault-chips">
        {backup.sections.map(section => (
          <span key={section.name} className={`vault-chip ${section.name === 'ACCOUNTS' ? 'vault-chip-warn' : ''}`}>
            {section.name === 'ACCOUNTS' && <KeyRound size={11} />}
            {sectionName(section.name)}
            <strong>{section.count.toLocaleString()}</strong>
          </span>
        ))}
      </div>

      <div className="row-sub">
        {formatBytes(backup.sealedBytes)} stored
        {backup.plainBytes > 0 && ` · ${formatBytes(backup.plainBytes)} before compression (${saved}% smaller)`}
        {' · '}<code title={backup.sha256}>{backup.sha256.slice(0, 12)}</code>
      </div>
      {backup.includesAccounts && (
        <div className="row-sub vault-warn">
          Includes sign-ins. They are sealed like everything else, but your passphrase is what protects
          them if this server's database is ever copied.
        </div>
      )}
    </div>
  );
}
