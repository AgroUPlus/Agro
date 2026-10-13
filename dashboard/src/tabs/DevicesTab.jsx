import { useCallback, useEffect, useState } from 'react';
import { ArchiveRestore, KeyRound, MonitorSmartphone } from 'lucide-react';
import { gql } from '../api.js';
import SubTabs from '../expressive/SubTabs.jsx';
import { useSubPage } from '../expressive/useSubPage.js';
import DeviceGrid from './devices/DeviceGrid.jsx';
import SignIns from './devices/SignIns.jsx';
import BackupsTab from './BackupsTab.jsx';

const APP_PASSWORDS_QUERY = `query Devices($user: String!) {
  appPasswords(userId: $user) { id label createdAt lastUsedAt }
}`;

const MINT_MUTATION = `mutation MintToken($user: String!, $label: String!) {
  createAppPassword(userId: $user, label: $label) { token label }
}`;

const REVOKE_MUTATION = `mutation Revoke($user: String!, $id: Int!) {
  revokeAppPassword(userId: $user, id: $id)
}`;

const PAGES = [
  { id: 'devices', label: 'Devices', icon: MonitorSmartphone },
  { id: 'keys', label: 'Sign-ins', icon: KeyRound },
  { id: 'backups', label: 'Backups', icon: ArchiveRestore }
];

/**
 * Everything that belongs to the account's devices: the players themselves, the credentials they
 * sign in with, and the backups they send. Backups had a tab of their own; they come from these
 * devices, so they live here now and an old `#/backups` link opens them.
 */
export default function DevicesTab({ username, nodes = [], onRenameNode, onDeleteNode, onUnauthorized }) {
  const [page, openPage] = useSubPage('devices', PAGES.map(p => p.id), { backups: 'backups' });
  const [appPasswords, setAppPasswords] = useState([]);
  const [busy, setBusy] = useState(null);
  const [notice, setNotice] = useState('');

  const load = useCallback(async () => {
    try {
      const res = await gql(APP_PASSWORDS_QUERY, { user: username });
      const body = await res.json();
      setAppPasswords(body?.data?.appPasswords ?? []);
    } catch (err) {
      if (err.unauthorized) onUnauthorized?.();
    }
  }, [username, onUnauthorized]);

  useEffect(() => { load(); }, [load]);

  async function pair(label) {
    try {
      const res = await gql(MINT_MUTATION, { user: username, label });
      const body = await res.json();
      if (body?.errors?.length) throw new Error(body.errors[0].message);
      load();
      return body?.data?.createAppPassword ?? null;
    } catch (e) {
      if (e.unauthorized) onUnauthorized?.();
      else alert(e.message);
      return null;
    }
  }

  async function revoke(device) {
    if (!window.confirm(`Revoke “${device.label}”?\n\nThat device will be signed out immediately.`)) return;
    setBusy(device.id);
    setNotice('');
    try {
      const res = await gql(REVOKE_MUTATION, { user: username, id: device.id });
      const body = await res.json();
      if (body?.errors?.length) throw new Error(body.errors[0].message);
      setNotice(`Revoked “${device.label}”.`);
      await load();
    } catch (err) {
      if (err.unauthorized) onUnauthorized?.();
      else setNotice(err.message);
    } finally {
      setBusy(null);
    }
  }

  return (
    <>
      <SubTabs pages={PAGES} active={page} onOpen={openPage} />
      {page === 'devices' && (
        <DeviceGrid
          nodes={nodes}
          username={username}
          onRename={node => onRenameNode?.(node)}
          onDelete={id => onDeleteNode?.(id)}
          onPair={pair}
        />
      )}
      {page === 'keys' && (
        <SignIns appPasswords={appPasswords} busy={busy} notice={notice} onRevoke={revoke} />
      )}
      {page === 'backups' && <BackupsTab onUnauthorized={onUnauthorized} />}
    </>
  );
}
