import { useState } from 'react';

const VISIBILITY = [
  { value: 'PRIVATE', label: 'Private' },
  { value: 'FRIENDS', label: 'Friends' },
  { value: 'PUBLIC', label: 'Public' }
];

/** Edit access a visibility allows, mirroring `EditAccess::clamped_to` on the server. */
const ALLOWED = {
  PRIVATE: ['OFF'],
  FRIENDS: ['OFF', 'FRIENDS'],
  PUBLIC: ['OFF', 'FRIENDS', 'PUBLIC']
};

const COLLABORATION = [
  { value: 'OFF', label: 'Only me' },
  { value: 'FRIENDS', label: 'Friends can edit' },
  { value: 'PUBLIC', label: 'Friends edit, anyone adds' }
];

/** Who can open the playlist and who can change it. The owner's alone. */
export function SharingSelects({ playlist, onVisibility, onEditAccess, disabled }) {
  return (
    <>
      <select
        className="detail-select"
        aria-label="Who can open it"
        value={playlist.visibility}
        disabled={disabled}
        onChange={(event) => onVisibility(event.target.value)}
      >
        {VISIBILITY.map((option) => (
          <option key={option.value} value={option.value}>{option.label}</option>
        ))}
      </select>
      <select
        className="detail-select"
        aria-label="Who can edit it"
        value={playlist.editAccess}
        disabled={disabled}
        title={playlist.visibility === 'PRIVATE' ? 'Share it first to let others edit' : undefined}
        onChange={(event) => onEditAccess(event.target.value)}
      >
        {COLLABORATION.map((option) => (
          <option
            key={option.value}
            value={option.value}
            disabled={!ALLOWED[playlist.visibility].includes(option.value)}
          >
            {option.label}
          </option>
        ))}
      </select>
    </>
  );
}

/** Rename and describe. Sent with the revision it was opened at, like any other edit. */
export function DetailsForm({ playlist, onSave, onCancel }) {
  const [title, setTitle] = useState(playlist.title);
  const [description, setDescription] = useState(playlist.description || '');

  return (
    <form
      className="detail-panel"
      onSubmit={(event) => {
        event.preventDefault();
        if (title.trim()) onSave(title.trim(), description.trim());
      }}
    >
      <strong>Name &amp; details</strong>
      <input value={title} maxLength={255} onChange={(event) => setTitle(event.target.value)} aria-label="Name" />
      <textarea
        rows={3}
        value={description}
        maxLength={1024}
        placeholder="Add an optional description"
        onChange={(event) => setDescription(event.target.value)}
        aria-label="Description"
      />
      <div className="lib-section-actions">
        <button type="submit" className="pill-btn primary" disabled={!title.trim()}>Save</button>
        <button type="button" className="pill-btn" onClick={onCancel}>Cancel</button>
      </div>
    </form>
  );
}
