import React from 'react';
import { Save, Check } from 'lucide-react';

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

      <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: '12px', marginTop: '6px' }}>
        <div>
          <label className="form-label">Subsonic / Navidrome URL</label>
          <input
            type="text"
            className="form-input"
            value={synced.serverUrl}
            onChange={(e) => onSyncedChange({ ...synced, serverUrl: e.target.value })}
          />
        </div>
        <div>
          <label className="form-label">Server Username</label>
          <input
            type="text"
            className="form-input"
            value={synced.serverUsername}
            onChange={(e) => onSyncedChange({ ...synced, serverUsername: e.target.value })}
          />
        </div>
        <div>
          <label className="form-label">LRCLIB Synced Lyrics Endpoint</label>
          <input
            type="text"
            className="form-input"
            value={synced.lrclibUrl}
            onChange={(e) => onSyncedChange({ ...synced, lrclibUrl: e.target.value })}
          />
        </div>
        <div>
          <label className="form-label">Streaming Audio Quality</label>
          <select
            className="form-input"
            value={synced.streamFormat}
            onChange={(e) => onSyncedChange({ ...synced, streamFormat: e.target.value })}
          >
            <option value="FLAC">FLAC (Lossless Master)</option>
            <option value="OPUS">Opus (High Efficiency)</option>
            <option value="MP3">MP3 320k (Universal)</option>
          </select>
        </div>
      </div>
    </form>
  );
}
