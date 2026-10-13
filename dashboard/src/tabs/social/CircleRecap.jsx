import { Mic2, Music2, Trophy, Users } from 'lucide-react';
import Avatar from '../../Avatar.jsx';
import Cookie from '../../expressive/Cookie.jsx';
import { periodNoun } from './query.js';
import { splitTitle } from '../stats/format.js';

/**
 * The circle's period in one page: who is in it, the song it was about, who heard the hits first,
 * what it played, and how alike each pair of friends' taste is.
 */
export default function CircleRecap({ recap, loaded }) {
  if (!recap) {
    return (
      <div className="empty-state">
        <Cookie shape="clover" size={88}><Users size={34} /></Cookie>
        <span className="empty-state-title">{loaded ? 'Nothing to recap yet' : 'Loading…'}</span>
      </div>
    );
  }

  return (
    <>
      <div className="circle-top">
        {recap.anthem && (
          <div className="card circle-anthem">
            <span className="stats-overline">Anthem of the {periodNoun(recap.period)}</span>
            <div className="stats-top-song">
              <Cookie shape="flower" size={72}><Music2 size={30} /></Cookie>
              <div className="stats-top-song-text">
                <span className="stats-top-title">{recap.anthem.title}</span>
                <span className="stats-top-artist">{recap.anthem.artist}</span>
              </div>
              <span className="stats-top-count">×{recap.anthem.plays}</span>
            </div>
            <div className="circle-members">
              {recap.members.map(member => (
                <span key={member} className="circle-member" title={`@${member}`}>
                  <Avatar username={member} size={34} />
                </span>
              ))}
              <span className="circle-count">{recap.members.length} in the circle</span>
            </div>
          </div>
        )}

        {recap.trendsetter && (
          <div className="card circle-trend">
            <Cookie shape="sunny" size={84} className="trend-shape"><Trophy size={34} /></Cookie>
            <span className="stats-overline">Trendsetter</span>
            <span className="circle-trend-name">@{recap.trendsetter.username}</span>
            <span className="circle-trend-sub">First on {recap.trendsetter.firsts} of the top songs</span>
          </div>
        )}
      </div>

      <div className="circle-lists">
        <RankCard title="Songs" icon={Music2} entries={recap.topTracks} />
        <RankCard title="Artists" icon={Mic2} entries={recap.topArtists} />
        <div className="card">
          <div className="card-title"><Users size={18} />Taste match</div>
          {recap.matrix.length === 0 ? (
            <div className="empty-hint">Nobody to compare yet.</div>
          ) : (
            <ul className="match-list">
              {recap.matrix.map(entry => (
                <li key={`${entry.a}-${entry.b}`} className="match-row">
                  <span className="match-pair">
                    <Avatar username={entry.a} size={30} />
                    <Avatar username={entry.b} size={30} />
                  </span>
                  <span className="match-bar"><span style={{ width: `${entry.score}%` }} /></span>
                  <span className="match-score">{entry.score}%</span>
                </li>
              ))}
            </ul>
          )}
        </div>
      </div>
    </>
  );
}

function RankCard({ title, icon: Icon, entries }) {
  const peak = entries[0]?.value || 1;
  return (
    <div className="card">
      <div className="card-title"><Icon size={18} />{title}</div>
      {entries.length === 0 ? (
        <div className="empty-hint">Nothing yet.</div>
      ) : (
        <ol className="rank-list">
          {entries.slice(0, 5).map((entry, index) => {
            const { title: name, artist } = splitTitle(entry.name);
            return (
              <li key={entry.name} className="rank-row">
                <span className="rank-fill" style={{ width: `${(entry.value / peak) * 100}%` }} />
                <span className="rank-n">{index + 1}</span>
                <span className="rank-text">
                  <span className="rank-name">{name}</span>
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
}
