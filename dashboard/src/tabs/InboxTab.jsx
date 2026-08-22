import { useCallback, useEffect, useState } from 'react';
import { Archive, Inbox, Send, Music2 } from 'lucide-react';
import { gql } from '../App.jsx';
import { formatWhen } from './FeedTab.jsx';
import Avatar from '../Avatar.jsx';

/**
 * Songs your friends have handed you, and the ones you have handed out.
 *
 * The dashboard cannot play any of them — playback lives in Wander and Wanda — so this is a reader
 * and a tidier, not a player. Opening a drop marks it read; archiving takes it out of the list
 * without destroying the sender's record of having sent it.
 *
 * What the sent list deliberately does *not* show is whether anything was read. The server blanks
 * that field for senders, and this asks for it only on the received side, so the omission is
 * visible here rather than being a silent property of the API.
 */
const INBOX_QUERY = `query Inbox {
  inbox(limit: 100) {
    id fromUser trackTitle artistName albumName note createdAt readAt contentHash trackUri
  }
  unreadDropCount
}`;

const SENT_QUERY = `query SentDrops {
  sentDrops(limit: 100) {
    id toUser trackTitle artistName albumName note createdAt
  }
}`;

const MARK_READ = `mutation MarkRead($id: String!) { markDropRead(id: $id) }`;
const ARCHIVE = `mutation ArchiveDrop($id: String!) { archiveDrop(id: $id) }`;

export default function InboxTab({ onUnauthorized }) {
  const [view, setView] = useState('received');
  const [drops, setDrops] = useState([]);
  const [sent, setSent] = useState([]);
  const [busy, setBusy] = useState(null);
  const [loaded, setLoaded] = useState(false);

  const load = useCallback(async () => {
    try {
      const [inboxRes, sentRes] = await Promise.all([gql(INBOX_QUERY), gql(SENT_QUERY)]);
      const inboxBody = await inboxRes.json();
      const sentBody = await sentRes.json();
      setDrops(inboxBody?.data?.inbox ?? []);
      setSent(sentBody?.data?.sentDrops ?? []);
    } catch (error) {
      if (error.unauthorized) onUnauthorized?.();
    } finally {
      setLoaded(true);
    }
  }, [onUnauthorized]);

  useEffect(() => {
    load();
  }, [load]);

  const act = useCallback(
    async (query, id) => {
      setBusy(id);
      try {
        await gql(query, { id });
        await load();
      } catch (error) {
        if (error.unauthorized) onUnauthorized?.();
      } finally {
        setBusy(null);
      }
    },
    [load, onUnauthorized]
  );

  const unread = drops.filter(drop => !drop.readAt).length;

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: '16px' }}>
      <div className="card">
        <div className="card-header">
          <div>
            <div className="card-title">
              {view === 'received' ? 'Sent to you' : 'Sent by you'}
            </div>
            <div className="card-subtitle">
              {view === 'received'
                ? `${unread} unread`
                : 'Whether these were read is not reported — deliberately'}
            </div>
          </div>
          <div className="segmented">
            <button
              className={`segmented-btn${view === 'received' ? ' active' : ''}`}
              onClick={() => setView('received')}
            >
              Received
            </button>
            <button
              className={`segmented-btn${view === 'sent' ? ' active' : ''}`}
              onClick={() => setView('sent')}
            >
              Sent
            </button>
          </div>
        </div>

        {view === 'received' ? (
          drops.length === 0 ? (
            <div className="empty-hint">
              {loaded ? 'Nothing yet. A friend can drop you a track from Wander or Wanda.' : 'Reading…'}
            </div>
          ) : (
            <div className="feed-list">
              {drops.map(drop => (
                <div key={drop.id} className={`drop-row${drop.readAt ? '' : ' unread'}`} style={{ display: 'flex', alignItems: 'center', gap: '12px' }}>
                  <Avatar username={drop.fromUser} size={32} />
                  <div className="feed-body" style={{ flex: 1 }}>
                    <div className="feed-summary">
                      {drop.trackTitle} — {drop.artistName}
                    </div>
                    {drop.note && <div className="drop-note">“{drop.note}”</div>}
                    <div className="feed-meta">
                      from {drop.fromUser} · {formatWhen(drop.createdAt)}
                      {drop.contentHash && ' · in the library'}
                    </div>
                  </div>
                  <div className="drop-actions">
                    {!drop.readAt && (
                      <button
                        className="btn btn-secondary"
                        disabled={busy === drop.id}
                        onClick={() => act(MARK_READ, drop.id)}
                      >
                        <Inbox size={13} /> Mark read
                      </button>
                    )}
                    <button
                      className="btn btn-secondary"
                      disabled={busy === drop.id}
                      onClick={() => act(ARCHIVE, drop.id)}
                    >
                      <Archive size={13} /> Archive
                    </button>
                  </div>
                </div>
              ))}
            </div>
          )
        ) : sent.length === 0 ? (
          <div className="empty-hint">{loaded ? 'You have not sent anything yet.' : 'Reading…'}</div>
        ) : (
          <div className="feed-list">
            {sent.map(drop => (
              <div key={drop.id} className="drop-row" style={{ display: 'flex', alignItems: 'center', gap: '12px' }}>
                <Avatar username={drop.toUser} size={32} />
                <div className="feed-body" style={{ flex: 1 }}>
                  <div className="feed-summary">
                    {drop.trackTitle} — {drop.artistName}
                  </div>
                  {drop.note && <div className="drop-note">“{drop.note}”</div>}
                  <div className="feed-meta">
                    to {drop.toUser} · {formatWhen(drop.createdAt)}
                  </div>
                </div>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
