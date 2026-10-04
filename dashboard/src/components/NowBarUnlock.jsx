import React, { useState } from 'react';
import { Lock, Loader2 } from 'lucide-react';
import { gql } from '../api.js';
import { unlockPresence } from '../vault.js';

/**
 * Opens a private session in a tab that has no key yet — one signed in before this existed, or
 * opened since: the key lives in `sessionStorage`, so each tab asks once. The passphrase goes no
 * further than this browser; the server only hands back the envelope it already keeps.
 */
export default function NowBarUnlock({ onUnlocked }) {
  const [open, setOpen] = useState(false);
  const [passphrase, setPassphrase] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');

  async function unlock(event) {
    event.preventDefault();
    setBusy(true);
    setError('');
    try {
      const res = await gql(`query { vaultKeyEnvelope { vaultSalt vaultKeyWrapped } }`);
      const envelope = (await res.json())?.data?.vaultKeyEnvelope;
      if (!envelope) throw new Error('No device has set up a vault key yet');
      await unlockPresence(passphrase, envelope.vaultSalt, envelope.vaultKeyWrapped);
      setPassphrase('');
      setOpen(false);
      onUnlocked?.();
    } catch (err) {
      // A wrong passphrase fails inside AES-GCM with an opaque error; say what it almost always is.
      setError(err instanceof DOMException ? 'That passphrase does not open your vault' : err.message);
    } finally {
      setBusy(false);
    }
  }

  if (!open) {
    return (
      <button className="btn btn-secondary" style={{ padding: '4px 10px' }} onClick={() => setOpen(true)}>
        <Lock size={12} />
        <span>Unlock</span>
      </button>
    );
  }

  return (
    <form onSubmit={unlock} style={{ display: 'flex', alignItems: 'center', gap: '6px' }}>
      <input
        type="password"
        autoFocus
        autoComplete="current-password"
        placeholder="Passphrase"
        value={passphrase}
        onChange={(e) => setPassphrase(e.target.value)}
        style={{ width: '160px' }}
      />
      <button className="btn btn-secondary" style={{ padding: '4px 10px' }} disabled={busy || !passphrase.trim()}>
        {busy ? <Loader2 size={12} className="auth-spin" /> : 'Open'}
      </button>
      {error && <span style={{ color: 'var(--status-error, #ef4444)', fontSize: '11px' }}>{error}</span>}
    </form>
  );
}
