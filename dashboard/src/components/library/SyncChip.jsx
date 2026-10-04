import { Check, CloudOff, Loader, RefreshCw, Ban } from 'lucide-react';

const STATES = {
  synced: { icon: Check, label: 'Synced' },
  saving: { icon: Loader, label: 'Saving…' },
  stale: { icon: RefreshCw, label: 'Out of sync — refreshing' },
  error: { icon: CloudOff, label: 'Out of sync' },
  revoked: { icon: Ban, label: 'No longer shared' }
};

/** Where this copy stands against the server. Always visible, so a stale view is never mistaken for a current one. */
export default function SyncChip({ state, onRetry }) {
  const { icon: Icon, label } = STATES[state] ?? STATES.synced;
  const content = (
    <>
      <Icon size={12} />
      <span>{label}</span>
    </>
  );
  return state === 'error' && onRetry ? (
    <button type="button" className={`sync-chip ${state}`} onClick={onRetry} title="Retry">
      {content}
    </button>
  ) : (
    <span className={`sync-chip ${state}`}>{content}</span>
  );
}
