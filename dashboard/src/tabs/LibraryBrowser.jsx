import { useCallback, useEffect, useState } from 'react';
import { Disc3, Search, Download, Trash2 } from 'lucide-react';
import { gql } from '../api.js';

/**
 * The library as a wall of covers, with a device picker that greys out what that device is missing.
 *
 * The Library tab used to be four counters and three paragraphs explaining the architecture — true,
 * but no help at all with the actual question, which is "what does my phone not have yet". Picking
 * a device answers it visually: anything faded is absent from that device, and one click offers it.
 */
const BROWSE_QUERY = `query Browse(
  $user: String!, $kind: LibraryBrowseKind!, $device: String, $search: String, $offset: Int
) {
  libraryBrowse(
    userId: $user, kind: $kind, deviceId: $device, search: $search, limit: 120, offset: $offset
  ) {
    id title subtitle coverKey trackCount presentOnDevice sourceCount
  }
}`;

const DELETE_ITEM_MUTATION = `mutation DeleteItem($user: String!, $kind: LibraryBrowseKind!, $id: String!) {
  deleteLibraryItem(userId: $user, kind: $kind, id: $id)
}`;

const OFFER_MUTATION = `mutation Offer($user: String!, $device: String!) {
  offerSync(userId: $user, deviceId: $device)
}`;

const KINDS = [
  { value: 'ALBUM', label: 'Albums' },
  { value: 'ARTIST', label: 'Artists' },
  { value: 'TRACK', label: 'Tracks' }
];

const PAGE_SIZE = 120;

export default function LibraryBrowser({ username, devices, onUnauthorized }) {
  const [kind, setKind] = useState('ALBUM');
  const [device, setDevice] = useState('');
  const [search, setSearch] = useState('');
  const [items, setItems] = useState([]);
  const [page, setPage] = useState(0);
  const [loading, setLoading] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const res = await gql(BROWSE_QUERY, {
        user: username,
        kind,
        device: device || null,
        search: search.trim() || null,
        offset: page * PAGE_SIZE
      });
      const body = await res.json();
      setItems(body?.data?.libraryBrowse ?? []);
    } catch (error) {
      if (error.unauthorized) onUnauthorized?.();
    } finally {
      setLoading(false);
    }
  }, [username, kind, device, search, page, onUnauthorized]);

  // Debounced, because this fires on every keystroke of the search box and each one is a query
  // over the whole index.
  useEffect(() => {
    const timer = setTimeout(load, 250);
    return () => clearTimeout(timer);
  }, [load]);

  // A filter change means the old page number points into a different list.
  useEffect(() => setPage(0), [kind, device, search]);

  const missing = items.filter(item => !item.presentOnDevice).length;

  async function deleteItem(id) {
    if (!confirm('Are you sure you want to completely remove this from the server index?')) return;
    try {
      await gql(DELETE_ITEM_MUTATION, { user: username, kind, id });
      load();
    } catch (error) {
      if (error.unauthorized) onUnauthorized?.();
    }
  }

  async function offerMissing() {
    if (!device) return;
    try {
      await gql(OFFER_MUTATION, { user: username, device });
    } catch (error) {
      if (error.unauthorized) onUnauthorized?.();
    }
  }

  return (
    <div className="card">
      <div className="card-header">
        <div>
          <div className="card-title">Browse</div>
          <div className="card-subtitle">
            {device
              ? `${missing} of ${items.length} shown are missing from this device`
              : 'Pick a device to see what it is missing'}
          </div>
        </div>
        {device && missing > 0 && (
          <button className="pill-btn" onClick={offerMissing}>
            <Download size={13} />
            <span>Offer missing</span>
          </button>
        )}
      </div>

      <div className="browse-controls">
        <div className="segmented">
          {KINDS.map(option => (
            <button
              key={option.value}
              className={`segmented-btn ${kind === option.value ? 'active' : ''}`}
              onClick={() => setKind(option.value)}
            >
              {option.label}
            </button>
          ))}
        </div>

        <select
          className="browse-select"
          value={device}
          onChange={event => setDevice(event.target.value)}
        >
          <option value="">All devices</option>
          {devices.map(node => (
            <option key={node.deviceId} value={node.deviceId}>
              {node.petname}
            </option>
          ))}
        </select>

        <label className="browse-search">
          <Search size={13} />
          <input
            value={search}
            placeholder="Search artist, album or title"
            onChange={event => setSearch(event.target.value)}
          />
        </label>
      </div>

      {items.length === 0 ? (
        <div className="empty-hint">
          {loading ? 'Loading…' : 'Nothing here yet. Turn on Library Sync in Wanda or Wander.'}
        </div>
      ) : (
        <div className="cover-grid">
          {items.map(item => (
            <div
              key={`${kind}:${item.id}`}
              className={`cover-tile ${item.presentOnDevice ? '' : 'absent'}`}
              title={
                item.presentOnDevice
                  ? `${item.title} — ${item.subtitle}`
                  : `${item.title} — not on this device`
              }
            >
              <div className="cover-art">
                {item.coverKey ? (
                  <img src={`/api/v1/cover/${item.coverKey}`} alt="" loading="lazy" />
                ) : (
                  <Disc3 size={28} />
                )}
                {item.sourceCount === 0 ? (
                  <span className="cover-badge" style={{background: 'var(--danger)'}}>0 sources</span>
                ) : !item.presentOnDevice ? (
                  <span className="cover-badge">Not here</span>
                ) : null}
                {username === 'alpha' && (
                  <button 
                    className="cover-delete-btn" 
                    title="Remove from Server"
                    onClick={(e) => { e.stopPropagation(); deleteItem(item.id); }}
                  >
                    <Trash2 size={14} />
                  </button>
                )}
              </div>
              <div className="cover-title">{item.title}</div>
              <div className="cover-sub">
                {item.subtitle}
                {kind !== 'TRACK' ? ` · ${item.trackCount}` : ''}
              </div>

            </div>
          ))}
        </div>
      )}

      {(page > 0 || items.length === PAGE_SIZE) && (
        <div className="browse-pager">
          <button className="pill-btn" disabled={page === 0} onClick={() => setPage(p => p - 1)}>
            Previous
          </button>
          <span>Page {page + 1}</span>
          <button
            className="pill-btn"
            disabled={items.length < PAGE_SIZE}
            onClick={() => setPage(p => p + 1)}
          >
            Next
          </button>
        </div>
      )}
    </div>
  );
}
