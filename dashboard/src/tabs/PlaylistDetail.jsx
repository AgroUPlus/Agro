import { useState } from 'react';
import { ArrowDown, ArrowLeft, ArrowUp, Link2, Pencil, Plus, Trash2, UserMinus, UserPlus, Users, X } from 'lucide-react';
import Avatar from '../Avatar.jsx';
import usePlaylistSync from '../hooks/usePlaylistSync.js';
import { deletePlaylist, follow, setEditAccess, setVisibility, unfollow } from '../playlistApi.js';
import DetailHero from '../components/library/DetailHero.jsx';
import PlaylistCover from '../components/library/PlaylistCover.jsx';
import { itemArt } from '../components/library/art.js';
import SyncChip from '../components/library/SyncChip.jsx';
import TrackTable from '../components/library/TrackTable.jsx';
import AddTracksPanel from '../components/library/AddTracksPanel.jsx';
import { DetailsForm, SharingSelects } from '../components/library/OwnerControls.jsx';
import { VISIBILITY_LABEL, formatTotal, songCount } from '../components/library/format.js';

const CAN_ADD = ['OWNER', 'EDITOR', 'CONTRIBUTOR'];
const CAN_REARRANGE = ['OWNER', 'EDITOR'];

/**
 * A playlist page, laid out the way a streaming app's is, offering only what the caller may do.
 *
 * Owner: who can open it, who can edit it, name and details, delete. Editor: add, remove,
 * reorder. Contributor: add, and remove what they added. Everyone else: follow and copy the link.
 */
export default function PlaylistDetail({ id, me, onBack, onUnauthorized }) {
  const sync = usePlaylistSync(id, onUnauthorized);
  const { playlist, status, banner } = sync;
  const [panel, setPanel] = useState(null);
  const [copied, setCopied] = useState(false);

  if (!playlist) {
    return (
      <div>
        <button type="button" className="pill-btn detail-back" onClick={onBack}><ArrowLeft size={14} /> Library</button>
        <div className="empty-hint">{status === 'revoked' ? 'This playlist is gone, or no longer shared with you.' : 'Loading…'}</div>
      </div>
    );
  }

  const role = playlist.myRole;
  const collaborative = playlist.editAccess !== 'OFF';
  const canRearrange = CAN_REARRANGE.includes(role);
  const items = playlist.items;
  const locked = status === 'saving' || status === 'revoked';

  const remove = (row) => sync.commit([{ remove: row.key }]);
  const move = (from, to) => {
    const order = items.filter((_, index) => index !== from);
    const afterItemId = to === 0 ? null : order[to - 1].id;
    sync.commit([{ move: { itemId: items[from].id, afterItemId } }]);
  };

  const copyLink = async () => {
    const url = `${window.location.origin}/listen?pl=${playlist.id}`;
    // The clipboard API exists only on a secure origin; a dashboard reached over plain HTTP on the
    // LAN gets the link to copy by hand instead.
    if (!navigator.clipboard) {
      window.prompt('Copy this link', url);
      return;
    }
    await navigator.clipboard.writeText(url);
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };

  const removeOne = async () => {
    if (!window.confirm(`Delete "${playlist.title}"? Everyone following it loses it too.`)) return;
    try {
      await deletePlaylist(playlist.id);
      onBack();
    } catch (failure) {
      if (failure.unauthorized) onUnauthorized?.();
      else window.alert(`Couldn't delete it: ${failure.message}`);
    }
  };

  const rows = items.map((item) => ({
    key: item.id,
    title: item.title,
    artist: item.artist,
    album: item.album,
    art: itemArt(item),
    addedAt: item.addedAt,
    addedBy: item.addedBy,
    durationMs: item.durationMs
  }));

  return (
    <div>
      <button type="button" className="pill-btn detail-back" onClick={onBack}><ArrowLeft size={14} /> Library</button>

      <DetailHero
        cover={<PlaylistCover items={items} size={56} />}
        kind={
          <>
            <span>{VISIBILITY_LABEL[playlist.visibility]}</span>
            {collaborative && <span><Users size={12} /> Collaborative</span>}
            <SyncChip state={status} onRetry={sync.reload} />
          </>
        }
        title={playlist.title}
        description={playlist.description}
        byline={
          <>
            <Avatar username={playlist.userId} size={24} />
            <strong>{playlist.userId === me ? 'You' : playlist.userId}</strong>
            <span>· {songCount(playlist.itemCount)}, {formatTotal(playlist.totalDurationMs)}</span>
          </>
        }
      />

      <div className="detail-actions">
        {CAN_ADD.includes(role) && (
          <button type="button" className="pill-btn primary" disabled={locked} onClick={() => setPanel('add')}>
            <Plus size={14} /> Add
          </button>
        )}
        {role === 'OWNER' && (
          <>
            <button type="button" className="pill-btn" disabled={locked} onClick={() => setPanel('details')}>
              <Pencil size={14} /> Name &amp; details
            </button>
            <SharingSelects
              playlist={playlist}
              disabled={locked}
              onVisibility={(v) => sync.change(() => setVisibility(playlist.id, v))}
              onEditAccess={(a) => sync.change(() => setEditAccess(playlist.id, a))}
            />
          </>
        )}
        {role !== 'OWNER' && (
          <button
            type="button"
            className="pill-btn"
            onClick={() => sync.change(() => (playlist.isFollowing ? unfollow : follow)(playlist.id))}
          >
            {playlist.isFollowing ? <><UserMinus size={14} /> Unfollow</> : <><UserPlus size={14} /> Follow</>}
          </button>
        )}
        {playlist.visibility !== 'PRIVATE' && (
          <button type="button" className="pill-btn" onClick={copyLink}>
            <Link2 size={14} /> {copied ? 'Copied' : 'Copy link'}
          </button>
        )}
        {role === 'OWNER' && (
          <button type="button" className="pill-btn danger" onClick={removeOne}><Trash2 size={14} /> Delete</button>
        )}
      </div>

      {banner && (
        <div className="detail-banner" role="status">
          <span>{banner}</span>
          <button type="button" className="icon-btn" onClick={sync.dismissBanner} aria-label="Dismiss"><X size={14} /></button>
        </div>
      )}

      {panel === 'add' && (
        <AddTracksPanel
          username={me}
          onUnauthorized={onUnauthorized}
          onClose={() => setPanel(null)}
          onAdd={(track) => sync.commit([{ add: { track, afterItemId: null } }])}
        />
      )}
      {panel === 'details' && (
        <DetailsForm
          playlist={playlist}
          onCancel={() => setPanel(null)}
          onSave={async (title, description) => {
            if (await sync.saveDetails(title, description)) setPanel(null);
          }}
        />
      )}

      {rows.length === 0 ? (
        <div className="empty-hint">Nothing here yet.</div>
      ) : (
        <TrackTable
          rows={rows}
          columns={{ album: true, added: true, by: collaborative }}
          onMove={canRearrange && !locked ? move : undefined}
          actions={(row, index) => (
            <>
              {canRearrange && (
                <>
                  <button type="button" className="icon-btn" title="Move up" disabled={locked || index === 0} onClick={() => move(index, index - 1)}>
                    <ArrowUp size={14} />
                  </button>
                  <button type="button" className="icon-btn" title="Move down" disabled={locked || index === rows.length - 1} onClick={() => move(index, index + 1)}>
                    <ArrowDown size={14} />
                  </button>
                </>
              )}
              {(canRearrange || (role === 'CONTRIBUTOR' && row.addedBy === me)) && (
                <button type="button" className="icon-btn" title="Remove" disabled={locked} onClick={() => remove(row)}>
                  <Trash2 size={14} />
                </button>
              )}
            </>
          )}
        />
      )}
    </div>
  );
}
