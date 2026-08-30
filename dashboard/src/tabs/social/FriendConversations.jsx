import React, { useState } from 'react';
import { Music2, Lock, Send } from 'lucide-react';
import Avatar from '../../Avatar.jsx';

function formatWhen(iso) {
  if (!iso) return '';
  const seconds = Math.max(0, (Date.now() - new Date(iso).getTime()) / 1000);
  if (seconds < 60) return 'just now';
  if (seconds < 3600) return `${Math.round(seconds / 60)}m ago`;
  if (seconds < 86400) return `${Math.round(seconds / 3600)}h ago`;
  return `${Math.round(seconds / 86400)}d ago`;
}

export default function FriendConversations({
  conversations = {},
  selectedFriend,
  onSelectFriend,
  onSendDrop,
  onMarkRead,
  busy
}) {
  const [dropTrackTitle, setDropTrackTitle] = useState('');
  const [dropArtistName, setDropArtistName] = useState('');
  const [dropNote, setDropNote] = useState('');

  const activeThread = selectedFriend ? (conversations[selectedFriend] || { messages: [], unread: 0 }) : null;

  const handleSubmit = (e) => {
    e.preventDefault();
    if (!selectedFriend || !dropTrackTitle.trim() || !dropArtistName.trim()) return;
    onSendDrop({
      toUser: selectedFriend,
      trackTitle: dropTrackTitle.trim(),
      artistName: dropArtistName.trim(),
      note: dropNote.trim() || null
    });
    setDropTrackTitle('');
    setDropArtistName('');
    setDropNote('');
  };

  return (
    <div style={{ display: 'grid', gridTemplateColumns: '260px 1fr', gap: '16px', minHeight: '520px' }}>
      {/* Left: Friend Threads */}
      <div className="card" style={{ padding: '12px', display: 'flex', flexDirection: 'column', gap: '8px' }}>
        <div className="card-title" style={{ fontSize: '13px', padding: '4px 6px' }}>Friends</div>
        {Object.keys(conversations).length === 0 ? (
          <div className="empty-hint" style={{ padding: '16px' }}>No friends or conversations yet.</div>
        ) : (
          <div style={{ display: 'flex', flexDirection: 'column', gap: '4px' }}>
            {Object.entries(conversations).map(([username, thread]) => {
              const isSelected = selectedFriend === username;
              const lastMsg = thread.messages[thread.messages.length - 1];
              return (
                <button
                  key={username}
                  className={`thread-item ${isSelected ? 'active' : ''}`}
                  onClick={() => onSelectFriend(username)}
                >
                  <Avatar username={username} size={32} />
                  <div className="thread-meta">
                    <div className="thread-header">
                      <span className="thread-name">{thread.friend.displayName || username}</span>
                      {thread.unread > 0 && <span className="unread-dot">{thread.unread}</span>}
                    </div>
                    <div className="thread-preview">
                      {lastMsg ? `${lastMsg.isMine ? 'You: ' : ''}${lastMsg.trackTitle}` : 'No drops yet'}
                    </div>
                  </div>
                </button>
              );
            })}
          </div>
        )}
      </div>

      {/* Right: Thread Conversation Detail */}
      <div className="card" style={{ display: 'flex', flexDirection: 'column', justifyContent: 'space-between', padding: '16px' }}>
        {selectedFriend ? (
          <>
            <div className="convo-header">
              <div style={{ display: 'flex', alignItems: 'center', gap: '10px' }}>
                <Avatar username={selectedFriend} size={28} />
                <div>
                  <div style={{ fontWeight: '600', fontSize: '14px' }}>{selectedFriend}</div>
                  <div style={{ fontSize: '11px', color: 'var(--text-muted)' }}>Threaded Track Drops</div>
                </div>
              </div>
            </div>

            {/* Message bubbles */}
            <div className="messages-stream">
              {activeThread?.messages.length === 0 ? (
                <div className="empty-hint" style={{ margin: 'auto', textAlign: 'center' }}>
                  No drops shared with @{selectedFriend} yet. Drop a track below!
                </div>
              ) : (
                activeThread?.messages.map((m) => (
                  <div key={m.id} className={`message-row ${m.isMine ? 'mine' : 'theirs'}`}>
                    <div className="message-bubble">
                      <div className="drop-card-header">
                        <Music2 size={14} style={{ color: 'var(--status-active)' }} />
                        <strong>{m.trackTitle}</strong> · <span>{m.artistName}</span>
                        {m.isEncrypted && (
                          <span className="e2ee-tag" title="End-to-End Encrypted">
                            <Lock size={10} /> E2EE
                          </span>
                        )}
                      </div>
                      {m.note && <div className="drop-note-text">“{m.note}”</div>}
                      <div className="drop-bubble-footer">
                        <span>{formatWhen(m.createdAt)}</span>
                        {!m.isMine && !m.readAt && (
                          <button className="mark-read-btn" onClick={() => onMarkRead(m.id)}>
                            Mark read
                          </button>
                        )}
                      </div>
                    </div>
                  </div>
                ))
              )}
            </div>

            {/* Compose bar */}
            <form onSubmit={handleSubmit} className="compose-drop-bar">
              <input
                type="text"
                placeholder="Track Title"
                value={dropTrackTitle}
                onChange={(e) => setDropTrackTitle(e.target.value)}
                style={{ flex: 1 }}
                required
              />
              <input
                type="text"
                placeholder="Artist Name"
                value={dropArtistName}
                onChange={(e) => setDropArtistName(e.target.value)}
                style={{ flex: 1 }}
                required
              />
              <input
                type="text"
                placeholder="Note (optional)"
                value={dropNote}
                onChange={(e) => setDropNote(e.target.value)}
                style={{ flex: 1.5 }}
              />
              <button type="submit" className="btn btn-primary" disabled={busy === 'sending'}>
                <Send size={13} />
                <span>Drop</span>
              </button>
            </form>
          </>
        ) : (
          <div className="empty-hint" style={{ margin: 'auto' }}>Select a friend to open their conversation.</div>
        )}
      </div>
    </div>
  );
}
