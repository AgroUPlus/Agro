import { useCallback, useEffect, useState } from 'react';
import { TrendingUp } from 'lucide-react';
import { PopularPodium, PopularRow } from './popular/PopularRows.jsx';

/**
 * What the fleet has been playing, blinded: the same chart the promo site's Charts page shows,
 * read from the same public endpoint so the two can never disagree, covers and rank movement
 * included. Nothing in it is tied to an account, and rows below the exposure floor are simply
 * absent — see `/api/v1/popular` on the server.
 */
const WINDOWS = [
  { label: 'Today', days: 2 },
  { label: '7 days', days: 7 },
  { label: '30 days', days: 30 }
];

export default function PopularTab() {
  const [days, setDays] = useState(7);
  const [chart, setChart] = useState(null);
  const [failed, setFailed] = useState(false);

  const load = useCallback(async () => {
    try {
      const res = await fetch(`/api/v1/popular?days=${days}&limit=20`);
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      setChart(await res.json());
      setFailed(false);
    } catch {
      setFailed(true);
    }
  }, [days]);

  useEffect(() => {
    load();
    const timer = setInterval(load, 60000);
    return () => clearInterval(timer);
  }, [load]);

  const tracks = chart?.tracks ?? [];
  const podium = tracks.slice(0, 3);

  return (
    <>
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

      {failed && <div className="empty-hint">Could not load the chart.</div>}
      {chart && !chart.enabled && <div className="empty-hint">An admin has turned the chart off.</div>}
      {chart?.enabled && tracks.length === 0 && (
        <div className="empty-hint">
          <TrendingUp size={16} /> Nothing has been played enough to chart yet.
        </div>
      )}

      {podium.length > 0 && <PopularPodium tracks={podium} compared={chart.compared} />}

      {tracks.length > 3 && (
        <div className="card">
          <ol className="chart-rows">
            {tracks.slice(3).map((track, index) => (
              <PopularRow key={`${track.artist}-${track.title}`} track={track} rank={index + 4} compared={chart.compared} />
            ))}
          </ol>
        </div>
      )}
    </>
  );
}
