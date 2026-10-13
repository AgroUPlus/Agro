import { useCallback, useEffect, useState } from 'react';
import { AlertTriangle } from 'lucide-react';
import { gql, getToken } from '../api.js';
import { BackupHero, BackupRow, BackupSlots } from './vault/BackupViews.jsx';

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
    id createdAt deviceId deviceName appVersion sealedBytes includesAccounts
    sections { name count }
  }
  vaultLimits { keepPerAccount maxSealedBytes }
}`;

const DELETE_MUTATION = `mutation DeleteBackup($id: String!) { deleteVaultBackup(id: $id) }`;

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
    <>
      {notice && (
        <div className="empty-hint backup-notice"><AlertTriangle size={16} />{notice}</div>
      )}

      {backups && !latest && !notice && (
        <div className="empty-hint">
          No backups yet. In Wanda: <strong>Settings → Backup → Back up to Agro</strong>.
        </div>
      )}

      {latest && (
        <div className="backup-top">
          <BackupHero backup={latest} busy={busy === latest.id} onDownload={download} onDelete={remove} />
          <BackupSlots used={backups.length} limits={limits} />
        </div>
      )}

      {older.length > 0 && (
        <div className="card">
          <div className="card-title">Earlier</div>
          <ol className="backup-rows">
            {older.map(b => (
              <BackupRow key={b.id} backup={b} busy={busy === b.id} onDownload={download} onDelete={remove} />
            ))}
          </ol>
        </div>
      )}
    </>
  );
}
