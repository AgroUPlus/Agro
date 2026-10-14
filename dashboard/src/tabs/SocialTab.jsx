import { useCallback, useEffect, useState } from 'react';
import { Activity, BarChart3, Users } from 'lucide-react';
import { gql } from '../api.js';
import SubTabs from '../expressive/SubTabs.jsx';
import { useSubPage } from '../expressive/useSubPage.js';
import { SOCIAL_DATA_QUERY } from './social/query.js';
import FriendsGrid from './social/FriendsGrid.jsx';
import ActivityFeed from './social/ActivityFeed.jsx';
import CircleRecap from './social/CircleRecap.jsx';

const WINDOWS = [
  { label: 'Last 7 days', days: 7 },
  { label: 'Last 14 days', days: 14 },
  { label: 'Last 30 days', days: 30 }
];

const PERIODS = [
  { id: 'WEEK', label: 'This week' },
  { id: 'MONTH', label: 'This month' },
  { id: 'YEAR', label: 'This year' },
  { id: 'ALL', label: 'All time' }
];

/** Friends, what they have been up to, and what the circle shares — each a page of this tab. */
export default function SocialTab({ onUnauthorized }) {
  const [page, openPage] = useSubPage('social', ['friends', 'activity', 'circle']);
  const [days, setDays] = useState(14);
  const [period, setPeriod] = useState('MONTH');

  const [friends, setFriends] = useState([]);
  const [feedItems, setFeedItems] = useState([]);
  const [recap, setRecap] = useState(null);
  const [loaded, setLoaded] = useState(false);

  const loadData = useCallback(async () => {
    try {
      const res = await gql(SOCIAL_DATA_QUERY, { days, limit: 60, period });
      const data = (await res.json())?.data;
      const rawFriends = data?.friends || [];
      setFriends(rawFriends.filter(f => f.profile?.friendState?.toLowerCase() === 'accepted'));
      setFeedItems(data?.friendActivity ?? []);
      setRecap(data?.circleRecap ?? null);
    } catch (err) {
      if (err.unauthorized) onUnauthorized?.();
    } finally {
      setLoaded(true);
    }
  }, [days, period, onUnauthorized]);

  useEffect(() => {
    loadData();
    const interval = setInterval(loadData, 20000);
    return () => clearInterval(interval);
  }, [loadData]);

  const listening = friends.filter(f => f.nowPlaying?.isPlaying).length;
  const pages = [
    { id: 'friends', label: listening ? `Friends · ${listening} live` : 'Friends', icon: Users },
    { id: 'activity', label: 'Activity', icon: Activity },
    { id: 'circle', label: 'Circle', icon: BarChart3 }
  ];

  return (
    <>
      <SubTabs pages={pages} active={page} onOpen={openPage}>
        {page === 'activity' && (
          <select className="pill-select" value={days} onChange={e => setDays(Number(e.target.value))} aria-label="Window">
            {WINDOWS.map(w => <option key={w.days} value={w.days}>{w.label}</option>)}
          </select>
        )}
        {page === 'circle' && (
          <select className="pill-select pill-select-primary" value={period} onChange={e => setPeriod(e.target.value)} aria-label="Period">
            {PERIODS.map(p => <option key={p.id} value={p.id}>{p.label}</option>)}
          </select>
        )}
      </SubTabs>

      {page === 'friends' && <FriendsGrid friends={friends} loaded={loaded} />}
      {page === 'activity' && <ActivityFeed items={feedItems} loaded={loaded} />}
      {page === 'circle' && <CircleRecap recap={recap} loaded={loaded} />}
    </>
  );
}
