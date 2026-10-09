import { useEffect, useState } from 'react';
import { ShieldCheck, X } from 'lucide-react';
import { gql } from './api.js';

const STATUS = `query { totpStatus { isEnabled isAvailable } }`;
const QUIET_KEY = 'agro_2fa_nudge_until';
const QUIET_DAYS = 7;

// Storage can be blocked (private windows, strict settings). The nudge then simply shows every visit.
function quietUntil() {
  try {
    return Number(localStorage.getItem(QUIET_KEY)) || 0;
  } catch {
    return 0;
  }
}
function rememberQuiet() {
  try {
    localStorage.setItem(QUIET_KEY, String(Date.now() + QUIET_DAYS * 86_400_000));
  } catch {
    // Nothing to recover: the banner stays closed for this page view regardless.
  }
}

/**
 * A strong, dismissible suggestion to turn on two-factor sign-in. Never a gate: the account works
 * fully without it. Hidden once it is on, and for a week after "Not now".
 *
 * The server makes its own key on first run, so the factor is normally available. If it could not
 * (a read-only data folder) the banner says so instead of offering a button that would fail.
 */
export default function SecurityNudge({ isAdmin, onOpenSettings }) {
  const [status, setStatus] = useState(null);
  const [hidden, setHidden] = useState(() => quietUntil() > Date.now());

  useEffect(() => {
    if (hidden) return;
    gql(STATUS)
      .then((res) => res.json())
      .then((body) => setStatus(body?.data?.totpStatus ?? null))
      .catch(() => setStatus(null));
  }, [hidden]);

  if (hidden || !status || status.isEnabled) return null;

  function dismiss() {
    rememberQuiet();
    setHidden(true);
  }

  return (
    <div className="nudge" role="note">
      <ShieldCheck size={20} className="nudge-icon" />
      <div className="nudge-text">
        <strong>Protect your account with two-factor sign-in.</strong>
        {status.isAvailable ? (
          <span> It takes a minute and stops a leaked passphrase from being enough.</span>
        ) : isAdmin ? (
          <span> The server could not create its key file. Check that its data folder is writable, or set <code>AGRO_SECRET_KEY</code> yourself.</span>
        ) : (
          <span> This server is not set up for it yet. Ask its administrator.</span>
        )}
      </div>
      {status.isAvailable && (
        <button type="button" className="btn btn-primary nudge-action" onClick={onOpenSettings}>
          Set it up
        </button>
      )}
      <button type="button" className="nudge-close" onClick={dismiss} aria-label="Not now">
        <X size={16} />
      </button>
    </div>
  );
}
