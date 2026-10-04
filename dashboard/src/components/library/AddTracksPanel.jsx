import { useEffect, useState } from 'react';
import { Plus, Search } from 'lucide-react';
import { coverUrl, searchTracks } from '../../playlistApi.js';

/**
 * Finds tracks in the account's library to add. Each add goes out on its own, so a long session
 * of adding never holds a large pending edit that someone else's change could make stale.
 */
export default function AddTracksPanel({ username, onAdd, onClose, onUnauthorized }) {
  const [search, setSearch] = useState('');
  const [results, setResults] = useState([]);
  const [error, setError] = useState('');

  useEffect(() => {
    const term = search.trim();
    if (!term) {
      setResults([]);
      return undefined;
    }
    // Debounced: every keystroke would otherwise be a query over the whole index.
    const timer = setTimeout(async () => {
      try {
        setResults(await searchTracks(username, term));
        setError('');
      } catch (failure) {
        if (failure.unauthorized) onUnauthorized?.();
        else setError(failure.message);
      }
    }, 250);
    return () => clearTimeout(timer);
  }, [search, username, onUnauthorized]);

  return (
    <div className="detail-panel">
      <div className="lib-section-head" style={{ marginBottom: 0 }}>
        <strong>Add to this playlist</strong>
        <button type="button" className="pill-btn" onClick={onClose}>Done</button>
      </div>
      <label className="browse-search">
        <Search size={13} />
        <input
          autoFocus
          value={search}
          placeholder="Search your library"
          onChange={(event) => setSearch(event.target.value)}
        />
      </label>
      {error && <div className="card-subtitle">{error}</div>}
      <div className="search-results">
        {results.map((track) => (
          <div key={track.id} className="search-result">
            <div className="track-main">
              <div className="track-thumb">
                {track.coverKey && <img src={coverUrl(track.coverKey)} alt="" loading="lazy" />}
              </div>
              <div style={{ minWidth: 0 }}>
                <div className="track-title">{track.title}</div>
                <div className="track-artist">{track.subtitle}</div>
              </div>
            </div>
            <button
              type="button"
              className="icon-btn"
              title="Add"
              onClick={() => onAdd({ title: track.title, artist: track.subtitle })}
            >
              <Plus size={16} />
            </button>
          </div>
        ))}
      </div>
    </div>
  );
}
