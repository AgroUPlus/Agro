import { useCallback, useEffect, useState } from 'react';
import { Trash2, Smartphone, RefreshCw } from 'lucide-react';
import { gql } from '../App.jsx';

/**
 * Every credential issued for this account, and a way to take each one back.
 *
 * The Pairing tab has always told people to "revoke it any time under app passwords" — a place
 * that did not exist. `appPasswords` and `revokeAppPassword` were on the server the whole time
 * with nothing calling them, so tokens accumulated invisibly: a client that logs in again on each
 * launch leaves one row per launch and no one could see it happening.
 *
 * Each row is keyed and revoked by `id`, not by label. Labels repeat — several rows genuinely are
 * all called `wander-desktop` — and revoking by label signed all of them out at once.
 */
const DEVICES_QUERY = `query Devices($user: String!) {
  appPasswords(userId: $user) { id label createdAt lastUsedAt }
}`;

const REVOKE = `mutation Revoke($user: String!, $id: Int!) {
  revokeAppPassword(userId: $user, id: $id)
}`;

/** "never", or how long ago — an exact timestamp says less here than an age does. */
function when(iso) {
  if (!iso) return 'never used';
  const seconds = Math.max(0, (Date.now() - new Date(iso).getTime()) / 1000);
  if (seconds < 90) return 'just now';
  if (seconds < 5400) return `${Math.round(seconds / 60)} min ago`;
  if (seconds < 172800) return `${Math.round(seconds / 3600)} h ago`;
  return `${Math.round(seconds / 86400)} d ago`;
}

export default function DevicesTab({ username, onUnauthorized }) {
  const [devices, setDevices] = useState([]);
  const [busy, setBusy] = useState(null);
  const [notice, setNotice] = useState('');

  const load = useCallback(async () => {
    try {
      const res = await gql(DEVICES_QUERY, { user: username });
      const body = await res.json();
      setDevices(body?.data?.appPasswords ?? []);
    } catch (err) {
      if (err.unauthorized) onUnauthorized?.();
    }
  }, [username, onUnauthorized]);

  useEffect(() => { load(); }, [load]);

  async function revoke(device) {
    // Revoking the credential this browser is holding signs you out of it. Worth a sentence, not a
    // prohibition — signing every other device out and keeping this one is a normal thing to want.
    const ok = window.confirm(
      `Revoke “${device.label}”?\n\nThat device is signed out immediately and has to pair again. ` +
      `If it is the browser you are using now, you will be signed out too.`
    );
    if (!ok) return;

    setBusy(device.id);
    setNotice('');
    try {
      const res = await gql(REVOKE, { user: username, id: device.id });
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

  // Same label more than once is the signature of a client re-logging in instead of keeping its
  // token. Naming it is more useful than silently listing eight identical rows.
  const counts = devices.reduce((acc, d) => ({ ...acc, [d.label]: (acc[d.label] || 0) + 1 }), {});
  const repeated = Object.entries(counts).filter(([, n]) => n > 2);

  return (
    <div className="card">
      <div className="card-header">
        <div>
          <div className="card-title">Sign-ins</div>
          <div className="card-subtitle">
            One credential per device. Revoking one signs out only that device.
          </div>
        </div>
        <button className="btn btn-secondary" onClick={load}>
          <RefreshCw size={13} />
          <span>Refresh</span>
        </button>
      </div>

      {notice && <div className="empty-hint" style={{ padding: '10px 16px' }}>{notice}</div>}

      {repeated.length > 0 && (
        <div className="empty-hint" style={{ padding: '10px 16px', lineHeight: 1.55 }}>
          {repeated.map(([label, n]) => (
            <div key={label}>
              <strong>{n}</strong> tokens are all called “{label}”. That usually means the client
              logs in again on every launch instead of keeping the token it was given. Revoking the
              old ones is safe — the one in use keeps working.
            </div>
          ))}
        </div>
      )}

      {devices.length === 0 ? (
        <p className="empty-hint" style={{ padding: '24px' }}>
          No device tokens yet. Issue one from the Pairing tab.
        </p>
      ) : (
        <div className="rules-list">
          {devices.map((device) => (
            <div key={device.id} className="rule-row">
              <div className="rule-info">
                <div className="rule-title">
                  <Smartphone size={13} style={{ marginRight: '6px', verticalAlign: '-2px' }} />
                  {device.label}
                </div>
                <div className="rule-desc">
                  Issued {new Date(device.createdAt).toLocaleString()} · {when(device.lastUsedAt)}
                </div>
              </div>
              <button
                className="btn btn-secondary"
                disabled={busy === device.id}
                onClick={() => revoke(device)}
                title="Revoke this credential"
              >
                <Trash2 size={13} />
                <span>{busy === device.id ? 'Revoking…' : 'Revoke'}</span>
              </button>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
