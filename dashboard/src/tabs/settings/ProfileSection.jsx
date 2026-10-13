import { Save, Check } from 'lucide-react';
import Avatar from '../../Avatar.jsx';
import PrivacyTiles from './PrivacyTiles.jsx';

/** How friends see you, beside who may see what. Saved together, as one form. */
export default function ProfileSection({
  username,
  profile,
  onProfileChange,
  visibility,
  onVisibilityChange,
  onSave,
  saved
}) {
  return (
    <form onSubmit={onSave} className="card profile-card">
      <div className="profile-row">
        <div className="profile-preview">
          <Avatar username={username} size={88} />
          <span className="profile-preview-name">{profile.displayName || username}</span>
          <span className="profile-preview-bio">{profile.bio || '@' + username}</span>
        </div>

        <div className="profile-fields">
          <label className="field">
            <span className="form-label">Name</span>
            <input
              type="text"
              className="form-input"
              value={profile.displayName}
              placeholder="Your name"
              onChange={(e) => onProfileChange({ ...profile, displayName: e.target.value })}
            />
          </label>
          <label className="field">
            <span className="form-label">Avatar link</span>
            <input
              type="url"
              className="form-input"
              value={profile.avatarUrl}
              placeholder="https://example.com/avatar.png"
              onChange={(e) => onProfileChange({ ...profile, avatarUrl: e.target.value })}
            />
          </label>
          <label className="field field-wide">
            <span className="form-label">Bio</span>
            <textarea
              className="form-input"
              rows={2}
              value={profile.bio}
              placeholder="A short note about your music taste"
              onChange={(e) => onProfileChange({ ...profile, bio: e.target.value })}
            />
          </label>
        </div>
      </div>

      <PrivacyTiles visibility={visibility} onChange={onVisibilityChange} />

      <div className="form-actions">
        <button type="submit" className="btn btn-primary">
          {saved ? <Check size={14} /> : <Save size={14} />}
          <span>{saved ? 'Saved' : 'Save'}</span>
        </button>
      </div>
    </form>
  );
}
