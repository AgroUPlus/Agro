import { useCallback, useEffect, useState } from 'react';
import { Disc3, Plus, Users } from 'lucide-react';
import { coverUrl, createPlaylist, listAlbums, listFollowed, listPlaylists } from '../playlistApi.js';
import PlaylistCover from '../components/library/PlaylistCover.jsx';

/**
 * The Library tab's front page: playlists first, then albums, each a wall of covers that opens a
 * full page. Playlists are the caller's own, those they follow, and those shared with them.
 */
export default function LibraryOverview({ username, onOpenPlaylist, onOpenAlbum, onBrowseAll, onUnauthorized }) {
  const [playlists, setPlaylists] = useState(null);
  const [albums, setAlbums] = useState(null);
  const [error, setError] = useState('');

  const load = useCallback(async () => {
    try {
      const [mine, followed, albumList] = await Promise.all([
        listPlaylists(),
        listFollowed(),
        listAlbums(username)
      ]);
      // Own first, then followed, then whatever else is shared: the order of how much it is yours.
      const seen = new Set();
      const ordered = [
        ...mine.filter((p) => p.userId === username),
        ...followed.playlists,
        ...mine.filter((p) => p.userId !== username)
      ].filter((p) => !seen.has(p.id) && seen.add(p.id));
      setPlaylists(ordered);
      setAlbums(albumList);
      setError('');
    } catch (failure) {
      if (failure.unauthorized) onUnauthorized?.();
      else setError(failure.message);
    }
  }, [username, onUnauthorized]);

  useEffect(() => {
    if (username) load();
  }, [username, load]);

  const newPlaylist = async () => {
    const title = window.prompt('Name the new playlist')?.trim();
    if (!title) return;
    try {
      onOpenPlaylist(await createPlaylist(title));
    } catch (failure) {
      if (failure.unauthorized) onUnauthorized?.();
      else setError(failure.message);
    }
  };

  const open = (handler) => ({
    role: 'button',
    tabIndex: 0,
    onClick: handler,
    onKeyDown: (event) => (event.key === 'Enter' || event.key === ' ') && handler()
  });

  return (
    <div className="card">
      {error && <div className="detail-banner" role="alert">{error}</div>}

      <section className="lib-section">
        <div className="lib-section-head">
          <span className="lib-section-title">Playlists</span>
          <div className="lib-section-actions">
            <button type="button" className="pill-btn" onClick={newPlaylist}><Plus size={13} /> New playlist</button>
          </div>
        </div>
        {playlists === null ? (
          <div className="empty-hint">Loading…</div>
        ) : playlists.length === 0 ? (
          <div className="empty-hint">No playlists yet. Share one from Wanda, or make one here.</div>
        ) : (
          <div className="cover-grid">
            {playlists.map((p) => (
              <div key={p.id} className="cover-tile clickable" {...open(() => onOpenPlaylist(p.id))}>
                <div className="cover-art">
                  <PlaylistCover items={p.items} />
                  {p.editAccess !== 'OFF' && <span className="cover-badge"><Users size={10} /> Collab</span>}
                </div>
                <div className="cover-title">{p.title}</div>
                <div className="cover-sub">{p.userId === username ? 'You' : p.userId} · {p.itemCount}</div>
              </div>
            ))}
          </div>
        )}
      </section>

      <section className="lib-section">
        <div className="lib-section-head">
          <span className="lib-section-title">Albums</span>
          <div className="lib-section-actions">
            <button type="button" className="pill-btn" onClick={onBrowseAll}>Browse everything</button>
          </div>
        </div>
        {albums === null ? (
          <div className="empty-hint">Loading…</div>
        ) : albums.length === 0 ? (
          <div className="empty-hint">Nothing here yet. Turn on Library Sync in Wanda or Wander.</div>
        ) : (
          <div className="cover-grid">
            {albums.map((album) => (
              <div key={album.id} className="cover-tile clickable" {...open(() => onOpenAlbum(album.id))}>
                <div className="cover-art">
                  {album.coverKey ? <img src={coverUrl(album.coverKey)} alt="" loading="lazy" /> : <Disc3 size={28} />}
                </div>
                <div className="cover-title">{album.title}</div>
                <div className="cover-sub">{album.subtitle} · {album.trackCount}</div>
              </div>
            ))}
          </div>
        )}
      </section>
    </div>
  );
}
