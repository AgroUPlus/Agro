import { useCallback, useEffect, useState } from 'react';
import { TrendingUp } from 'lucide-react';
import { gql } from '../api.js';

/**
 * What the fleet has been playing, blinded — see `submitPlayCounts`/`popularTracks` on the
 * server. Nothing here is tied to an account: rows below the exposure floor are simply absent,
 * and an admin-disabled chart comes back as an empty list rather than an error, so this component
 * never has to know *why* it has nothing to show.
 */
const POPULAR_TRACKS_QUERY = `query PopularTracks($days: Int!, $limit: Int!) {
  popularTracks(days: $days, limit: $limit) {
    title artist album count
  }
}`;

const WINDOWS = [
  { label: '24 hours', days: 1 },
  { label: '7 days', days: 7 },
  { label: '30 days', days: 30 }
];

export default function PopularTab({ onUnauthorized }) {
  const [days, setDays] = useState(7);
  const [tracks, setTracks] = useState(null);

  const load = useCallback(async () => {
    try {
      const res = await gql(POPULAR_TRACKS_QUERY, { days, limit: 25 });
      const body = await res.json();
      setTracks(body?.data?.popularTracks ?? []);
    } catch (error) {
      if (error.unauthorized) onUnauthorized?.();
    }
  }, [days, onUnauthorized]);

  useEffect(() => {
    load();
    const timer = setInterval(load, 60000);
    return () => clearInterval(timer);
  }, [load]);

  const peak = Math.max(1, ...(tracks || []).map(t => t.count));

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: '16px' }}>
      <div className="card">
        <div className="card-header">
          <div>
            <div className="card-title">Popular on Agro</div>
            <div className="card-subtitle">
              What the whole server has been playing — counted with no account attached
            </div>
          </div>
        </div>

        <div className="browse-controls">
          <div className="segmented">
            {WINDOWS.map(option => (
              <button
                key={option.days}
                className={`segmented-btn ${days === option.days ? 'active' : ''}`}
                onClick={() => setDays(option.days)}
              >
                {option.label}
              </button>
            ))}
          </div>
        </div>
      </div>

      <div className="card">
        {tracks === null ? (
          <div className="empty-hint">Loading…</div>
        ) : tracks.length === 0 ? (
          <div className="empty-hint">
            <TrendingUp size={15} style={{ marginRight: '6px', verticalAlign: '-2px' }} />
            Nothing to show yet — either the server has not played enough for anything to clear the
            exposure floor, or an admin has turned this chart off.
          </div>
        ) : (
          <div className="chart-list">
            {tracks.map((track, index) => (
              <div key={`${track.artist}-${track.title}`} className="chart-row">
                <div
                  className="chart-row-fill"
                  style={{ width: `${(track.count / peak) * 100}%` }}
                />
                <span className="chart-rank">{index + 1}</span>
                <div className="chart-info">
                  <span className="chart-title" title={track.title}>{track.title}</span>
                  <span className="chart-artist" title={track.artist}>
                    {track.artist}{track.album ? ` · ${track.album}` : ''}
                  </span>
                </div>
                <span className="chart-count">{track.count} plays</span>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
