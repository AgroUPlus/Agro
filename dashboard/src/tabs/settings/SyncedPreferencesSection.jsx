import React from 'react';
import { Save, Check, Lock } from 'lucide-react';

/**
 * The settings that sync between clients — the ones this server can actually read.
 *
 * The Subsonic URL, the server username and the lyrics endpoint used to have inputs here. They are
 * gone on purpose: migration 27 moved them into a blob the *client* seals under a key derived from
 * the account passphrase, and this server has no key to open it. It cannot show their values, and
 * the dashboard cannot write replacements without the vault key, which lives on Wanda and Wander.
 *
 * Showing empty boxes for them was worse than showing nothing — they read as "your settings were
 * lost", and saving would have written blanks over settings that are in fact intact.
 */
export default function SyncedPreferencesSection({
  synced,
  onSyncedChange,
  onSave,
  saved
}) {
  return (
    <form onSubmit={onSave} className="card">
      <div className="card-header">
        <div>
          <div className="card-title">Cross-Device Synced Preferences</div>
          <div className="card-subtitle">Synchronized across your Wander desktop and Wanda mobile apps</div>
        </div>
        <button type="submit" className="btn btn-primary">
          {saved ? <Check size={13} /> : <Save size={13} />}
          <span>{saved ? 'Synced!' : 'Save & Sync'}</span>
        </button>
      </div>

      <div className="sealed-note">
        <Lock size={13} />
        <div>
          <strong>
            Your music server address, username and lyrics endpoint are end-to-end encrypted
            {synced.hasServerUrl ? ' and set' : ''}.
          </strong>{' '}
          They are sealed with a key this server does not have, so they can only be viewed or
          changed from Wander or Wanda. Nothing here has been reset.
        </div>
      </div>

      <div className="settings-grid">
        <div>
          <label className="form-label" htmlFor="stream-format">Streaming Audio Quality</label>
          <select
            id="stream-format"
            className="form-input"
            value={synced.streamFormat}
            onChange={(e) => onSyncedChange({ ...synced, streamFormat: e.target.value })}
          >
            <option value="FLAC">FLAC (Lossless Master)</option>
            <option value="OPUS">Opus (High Efficiency)</option>
            <option value="MP3">MP3 320k (Universal)</option>
          </select>
        </div>

        <div>
          <label className="form-label" htmlFor="share-domain">Share Link Domain</label>
          <input
            id="share-domain"
            type="text"
            className="form-input"
            placeholder="frwd.top"
            value={synced.shareDomain || ''}
            onChange={(e) => onSyncedChange({ ...synced, shareDomain: e.target.value })}
          />
        </div>

        <div className="settings-grid-wide">
          <label className="form-label" htmlFor="share-hosts">
            Share Forwarding Allowlist
          </label>
          <input
            id="share-hosts"
            type="text"
            className="form-input"
            placeholder="music.example.com, another.example.com"
            value={synced.shareHosts || ''}
            onChange={(e) => onSyncedChange({ ...synced, shareHosts: e.target.value })}
          />
          <p className="form-hint">
            The only hosts <code>/listen</code> will forward to. Without an entry here that route
            would be an open redirect wearing your own domain.
          </p>
        </div>
      </div>

      <div className="settings-switches">
        <label className="switch-row">
          <input
            type="checkbox"
            checked={!!synced.shareEnabled}
            onChange={(e) => onSyncedChange({ ...synced, shareEnabled: e.target.checked })}
          />
          <span>Rewrite share links onto your own domain</span>
        </label>
        <label className="switch-row">
          <input
            type="checkbox"
            checked={!!synced.lyricsFetchOnline}
            onChange={(e) => onSyncedChange({ ...synced, lyricsFetchOnline: e.target.checked })}
          />
          <span>Fetch synced lyrics online</span>
        </label>
      </div>
    </form>
  );
}
