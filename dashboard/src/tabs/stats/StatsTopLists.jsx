import { Disc3, Mic2, Music2 } from 'lucide-react';
import { splitTitle } from './format.js';

const LISTS = [
  { key: 'topArtists', title: 'Artists', icon: Mic2 },
  { key: 'topAlbums', title: 'Albums', icon: Disc3 },
  { key: 'topTracks', title: 'Songs', icon: Music2 }
];

/** The top five of each, side by side, each row's bar its share of the leader. */
export default function StatsTopLists({ stats }) {
  return (
    <div className="stats-top-lists">
      {LISTS.map(({ key, title, icon: Icon }) => {
        const entries = stats[key].slice(0, 5);
        const peak = entries[0]?.value || 1;
        return (
          <div key={key} className="card">
            <div className="card-title"><Icon size={18} />{title}</div>
            {entries.length === 0 ? (
              <div className="empty-hint">Nothing yet.</div>
            ) : (
              <ol className="rank-list">
                {entries.map((entry, index) => {
                  const { title: name, artist } = splitTitle(entry.name);
                  return (
                    <li key={entry.name} className="rank-row">
                      <span className="rank-fill" style={{ width: `${(entry.value / peak) * 100}%` }} />
                      <span className="rank-n">{index + 1}</span>
                      <span className="rank-text">
                        <span className="rank-name" title={name}>{name}</span>
                        {artist && <span className="rank-sub">{artist}</span>}
                      </span>
                      <span className="rank-value">{entry.value}</span>
                    </li>
                  );
                })}
              </ol>
            )}
          </div>
        );
      })}
    </div>
  );
}
