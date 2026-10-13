import { Activity, Award, Repeat, Sparkles } from 'lucide-react';
import Avatar from '../../Avatar.jsx';
import Cookie from '../../expressive/Cookie.jsx';
import { formatWhen } from './query.js';

/** What each kind of event is drawn as: its icon on its own shape and colour. */
const KINDS = {
  MILESTONE: { icon: Award, shape: 'sunny', tone: 'lemon' },
  ON_REPEAT: { icon: Repeat, shape: 'clover', tone: 'tertiary' },
  NEW_FAVOURITE: { icon: Sparkles, shape: 'flower', tone: 'pink' }
};

/** Milestones, repeat plays and new favourites from friends who share them, newest first. */
export default function ActivityFeed({ items, loaded }) {
  if (items.length === 0) {
    return (
      <div className="empty-state">
        <Cookie shape="sunny" size={88}><Activity size={34} /></Cookie>
        <span className="empty-state-title">{loaded ? 'Quiet for now' : 'Loading…'}</span>
        {loaded && <span className="empty-state-sub">Milestones, repeats and new favourites show up here.</span>}
      </div>
    );
  }

  return (
    <div className="card">
      <ol className="feed-timeline">
        {items.map((item, index) => {
          const kind = KINDS[item.kind] ?? { icon: Activity, shape: 'cookie', tone: 'primary' };
          const Icon = kind.icon;
          return (
            <li key={`${item.username}-${item.at}-${index}`} className="feed-event">
              <span className="feed-avatar">
                <Avatar username={item.username} size={44} />
                <Cookie shape={kind.shape} size={26} className={`feed-badge tone-shape-${kind.tone}`}>
                  <Icon size={13} strokeWidth={2.6} />
                </Cookie>
              </span>
              <span className="feed-text">
                <span><b>@{item.username}</b> {item.summary || `listened to ${item.title}`}</span>
                <span className="feed-when">{item.artist ? `${item.artist} · ` : ''}{formatWhen(item.at)}</span>
              </span>
            </li>
          );
        })}
      </ol>
    </div>
  );
}
