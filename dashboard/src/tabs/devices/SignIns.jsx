import { KeyRound, Trash2 } from 'lucide-react';

function when(iso) {
  if (!iso) return 'never used';
  const seconds = Math.max(0, (Date.now() - new Date(iso).getTime()) / 1000);
  if (seconds < 90) return 'used just now';
  if (seconds < 5400) return `used ${Math.round(seconds / 60)} min ago`;
  if (seconds < 172800) return `used ${Math.round(seconds / 3600)} h ago`;
  return `used ${Math.round(seconds / 86400)} d ago`;
}

/** Every credential issued to the account, one per line, each revocable on its own. */
export default function SignIns({ appPasswords, busy, notice, onRevoke }) {
  return (
    <div className="card">
      {notice && <div className="inline-notice">{notice}</div>}
      {appPasswords.length === 0 ? (
        <div className="empty-hint">No sign-ins.</div>
      ) : (
        <ul className="key-rows">
          {appPasswords.map(device => (
            <li key={device.id} className="key-row">
              <span className="key-icon"><KeyRound size={18} /></span>
              <span className="key-text">
                <span className="key-label">{device.label}</span>
                <span className="key-sub">
                  Since {new Date(device.createdAt).toLocaleDateString()} · {when(device.lastUsedAt)}
                </span>
              </span>
              <button className="btn btn-danger" disabled={busy === device.id} onClick={() => onRevoke(device)}>
                <Trash2 size={14} />{busy === device.id ? 'Revoking…' : 'Revoke'}
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
