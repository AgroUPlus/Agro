import React from 'react';
import { Save, Check } from 'lucide-react';

export default function ProfileSection({
  profile,
  onProfileChange,
  visibility,
  onVisibilityChange,
  onSave,
  saved
}) {
  return (
    <form onSubmit={onSave} className="card">
      <div className="card-header">
        <div>
          <div className="card-title">Profile & Identity</div>
          <div className="card-subtitle">How your profile appears to friends on Wanda and Wander</div>
        </div>
        <button type="submit" className="btn btn-primary">
          {saved ? <Check size={13} /> : <Save size={13} />}
          <span>{saved ? 'Saved!' : 'Save Profile'}</span>
        </button>
      </div>

      <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: '14px', marginTop: '8px' }}>
        <div>
          <label className="form-label">Display Name</label>
          <input
            type="text"
            className="form-input"
            value={profile.displayName}
            placeholder="Your name"
            onChange={(e) => onProfileChange({ ...profile, displayName: e.target.value })}
          />
        </div>
        <div>
          <label className="form-label">Avatar URL (HTTP/HTTPS)</label>
          <input
            type="url"
            className="form-input"
            value={profile.avatarUrl}
            placeholder="https://example.com/avatar.png"
            onChange={(e) => onProfileChange({ ...profile, avatarUrl: e.target.value })}
          />
        </div>
        <div style={{ gridColumn: '1 / -1' }}>
          <label className="form-label">Bio</label>
          <textarea
            className="form-input"
            rows={2}
            value={profile.bio}
            placeholder="A short note about your music taste..."
            onChange={(e) => onProfileChange({ ...profile, bio: e.target.value })}
          />
        </div>
      </div>

      <div style={{ marginTop: '16px', borderTop: '1px solid var(--border-subtle)', paddingTop: '12px' }}>
        <div className="card-subtitle" style={{ marginBottom: '10px' }}>Privacy & Discovery Switches</div>
        <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: '10px' }}>
          <label className="checkbox-row">
            <input
              type="checkbox"
              checked={visibility.showNowPlaying}
              onChange={(e) => onVisibilityChange({ ...visibility, showNowPlaying: e.target.checked })}
            />
            <span>Broadcast Now Playing to Friends</span>
          </label>
          <label className="checkbox-row">
            <input
              type="checkbox"
              checked={visibility.showStats}
              onChange={(e) => onVisibilityChange({ ...visibility, showStats: e.target.checked })}
            />
            <span>Include in Friend Circle Stats & Recaps</span>
          </label>
          <label className="checkbox-row">
            <input
              type="checkbox"
              checked={visibility.showActivity}
              onChange={(e) => onVisibilityChange({ ...visibility, showActivity: e.target.checked })}
            />
            <span>Share Milestones in Social Activity Feed</span>
          </label>
          <label className="checkbox-row">
            <input
              type="checkbox"
              checked={visibility.discoverable}
              onChange={(e) => onVisibilityChange({ ...visibility, discoverable: e.target.checked })}
            />
            <span>Discoverable in User Search</span>
          </label>
        </div>
      </div>
    </form>
  );
}
