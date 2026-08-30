import React, { useCallback, useEffect, useState } from 'react';
import { Activity, MessageSquare } from 'lucide-react';
import { gql } from '../api.js';
import FriendConversations from './social/FriendConversations.jsx';
import ActivityFeed from './social/ActivityFeed.jsx';

const FEED_QUERY = `query Feed($days: Int, $limit: Int) {
  friendActivity(days: $days, limit: $limit) {
    username at kind summary artist title count
  }
}`;

const DROPS_CONVO_QUERY = `query DropsConvo {
  inbox(limit: 100) {
    id fromUser trackTitle artistName albumName note noteCiphertext isEncrypted createdAt readAt contentHash trackUri
  }
  sentDrops(limit: 100) {
    id toUser trackTitle artistName albumName note noteCiphertext isEncrypted createdAt contentHash trackUri
  }
  friends {
    profile {
      username displayName avatarUrl friendState
    }
  }
}`;

const DROP_MUTATION = `mutation DropTrack($toUser: String!, $trackUri: String!, $trackTitle: String!, $artistName: String!, $albumName: String, $note: String) {
  dropTrack(toUser: $toUser, trackUri: $trackUri, trackTitle: $trackTitle, artistName: $artistName, albumName: $albumName, note: $note) {
    id
  }
}`;

const MARK_READ = `mutation MarkRead($id: String!) { markDropRead(id: $id) }`;

export default function SocialTab({ me, onUnauthorized }) {
  const [activeSection, setActiveSection] = useState('conversations'); // 'conversations' | 'feed'
  const [selectedFriend, setSelectedFriend] = useState(null);
  const [inbox, setInbox] = useState([]);
  const [sent, setSent] = useState([]);
  const [friends, setFriends] = useState([]);
  const [feedItems, setFeedItems] = useState([]);
  const [busy, setBusy] = useState(null);

  const loadData = useCallback(async () => {
    try {
      const [dropsRes, feedRes] = await Promise.all([
        gql(DROPS_CONVO_QUERY),
        gql(FEED_QUERY, { days: 14, limit: 50 })
      ]);
      const dropsData = (await dropsRes.json())?.data;
      const feedData = (await feedRes.json())?.data;

      setInbox(dropsData?.inbox || []);
      setSent(dropsData?.sentDrops || []);
      const frList = (dropsData?.friends || [])
        .map(f => f.profile)
        .filter(Boolean)
        .filter(f => f.friendState?.toLowerCase() === 'accepted');
      setFriends(frList);
      setFeedItems(feedData?.friendActivity || []);

      if (!selectedFriend && frList.length > 0) {
        setSelectedFriend(frList[0].username);
      }
    } catch (err) {
      if (err.unauthorized) onUnauthorized?.();
    }
  }, [selectedFriend, onUnauthorized]);

  useEffect(() => { loadData(); }, [loadData]);

  // Group drops into conversation threads
  const conversations = {};
  friends.forEach(f => {
    conversations[f.username] = { friend: f, messages: [], unread: 0 };
  });

  inbox.forEach(item => {
    const user = item.fromUser;
    if (!conversations[user]) {
      conversations[user] = { friend: { username: user }, messages: [], unread: 0 };
    }
    conversations[user].messages.push({ ...item, isMine: false });
    if (!item.readAt) conversations[user].unread += 1;
  });

  sent.forEach(item => {
    const user = item.toUser;
    if (!conversations[user]) {
      conversations[user] = { friend: { username: user }, messages: [], unread: 0 };
    }
    conversations[user].messages.push({ ...item, isMine: true });
  });

  Object.values(conversations).forEach(c => {
    c.messages.sort((a, b) => new Date(a.createdAt).getTime() - new Date(b.createdAt).getTime());
  });

  const handleSendDrop = async ({ toUser, trackTitle, artistName, note }) => {
    try {
      setBusy('sending');
      await gql(DROP_MUTATION, {
        toUser,
        trackUri: `agro:track:${Date.now()}`,
        trackTitle,
        artistName,
        note
      });
      await loadData();
    } catch (err) {
      if (err.unauthorized) onUnauthorized?.();
      else alert(err.message);
    } finally {
      setBusy(null);
    }
  };

  const handleMarkRead = async (id) => {
    try {
      await gql(MARK_READ, { id });
      await loadData();
    } catch (err) {
      if (err.unauthorized) onUnauthorized?.();
    }
  };

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: '16px' }}>
      <div className="card" style={{ padding: '10px 16px' }}>
        <div className="segmented">
          <button
            className={`segmented-btn ${activeSection === 'conversations' ? 'active' : ''}`}
            onClick={() => setActiveSection('conversations')}
          >
            <MessageSquare size={14} style={{ marginRight: '6px' }} />
            Conversations
          </button>
          <button
            className={`segmented-btn ${activeSection === 'feed' ? 'active' : ''}`}
            onClick={() => setActiveSection('feed')}
          >
            <Activity size={14} style={{ marginRight: '6px' }} />
            Activity Feed
          </button>
        </div>
      </div>

      {activeSection === 'conversations' ? (
        <FriendConversations
          conversations={conversations}
          selectedFriend={selectedFriend}
          onSelectFriend={setSelectedFriend}
          onSendDrop={handleSendDrop}
          onMarkRead={handleMarkRead}
          busy={busy}
        />
      ) : (
        <ActivityFeed feedItems={feedItems} />
      )}
    </div>
  );
}
