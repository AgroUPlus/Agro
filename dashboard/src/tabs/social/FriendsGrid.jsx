import { EyeOff, Music2, UserPlus } from 'lucide-react';
import Avatar from '../../Avatar.jsx';
import Cookie from '../../expressive/Cookie.jsx';
import { formatWhen } from './query.js';

/**
 * Each friend as a card: who they are and, when they share it, what they are playing — with the
 * cover washed across the top of the card, so a glance at the grid says who is listening.
 */
export default function FriendsGrid({ friends, loaded }) {
  if (friends.length === 0) {
    return (
      <div className="empty-state">
        <Cookie shape="flower" size={88}><UserPlus size={34} /></Cookie>
        <span className="empty-state-title">{loaded ? 'No friends yet' : 'Loading…'}</span>
        {loaded && <span className="empty-state-sub">Add friends from Wanda or Wander with a friend code.</span>}
      </div>
    );
  }

  return (
    <div className="friend-grid">
      {friends.map(({ profile, nowPlaying }) => {
        const playing = nowPlaying?.isPlaying;
        const art = playing && nowPlaying.artworkUrl;
        return (
          <div key={profile.username} className={`friend-card ${playing ? 'is-playing' : ''}`}>
            {art && <div className="friend-art" style={{ backgroundImage: `url("${art.replace(/"/g, '%22')}")` }} />}
            <div className="friend-head">
              <Avatar username={profile.username} size={52} />
              <div className="friend-who">
                <span className="friend-name">{profile.displayName || profile.username}</span>
                <span className="friend-handle">@{profile.username}</span>
              </div>
            </div>
            {profile.bio && <p className="friend-bio">{profile.bio}</p>}

            {playing ? (
              <div className="friend-now">
                <div className="audio-pulse-indicator is-playing">
                  <span className="wave-bar bar-1" /><span className="wave-bar bar-2" />
                  <span className="wave-bar bar-3" /><span className="wave-bar bar-4" />
                </div>
                <span className="friend-now-text">
                  <span className="friend-now-title">{nowPlaying.trackTitle}</span>
                  <span className="friend-now-artist">{nowPlaying.artistName}</span>
                </span>
              </div>
            ) : (
              <div className="friend-idle">
                {profile.showNowPlaying
                  ? <><Music2 size={14} />{nowPlaying?.updatedAt ? `Last heard ${formatWhen(nowPlaying.updatedAt)}` : 'Not playing'}</>
                  : <><EyeOff size={14} />Keeps it private</>}
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
}
