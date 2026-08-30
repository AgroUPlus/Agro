import React from 'react';
import Avatar from '../../Avatar.jsx';

function formatWhen(iso) {
  if (!iso) return '';
  const seconds = Math.max(0, (Date.now() - new Date(iso).getTime()) / 1000);
  if (seconds < 60) return 'just now';
  if (seconds < 3600) return `${Math.round(seconds / 60)}m ago`;
  if (seconds < 86400) return `${Math.round(seconds / 3600)}h ago`;
  return `${Math.round(seconds / 86400)}d ago`;
}

export default function ActivityFeed({ feedItems = [] }) {
  return (
    <div className="card">
      <div className="card-header">
        <div>
          <div className="card-title">Friend Activity Feed</div>
          <div className="card-subtitle">Recent milestones, plays, and top tracks from your circle</div>
        </div>
      </div>
      {feedItems.length === 0 ? (
        <div className="empty-hint" style={{ padding: '24px' }}>No recent activity to show.</div>
      ) : (
        <div className="rules-list">
          {feedItems.map((item, i) => (
            <div key={i} className="rule-row">
              <Avatar username={item.username} size={28} />
              <div className="rule-info">
                <div className="rule-title">
                  <strong>@{item.username}</strong> {item.summary || `listened to ${item.title}`}
                </div>
                <div className="rule-desc">
                  {item.artist ? `${item.artist} · ` : ''}{formatWhen(item.at)}
                </div>
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
