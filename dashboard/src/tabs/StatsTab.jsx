import { useCallback, useEffect, useState } from 'react';
import { User, Users } from 'lucide-react';
import { gql } from '../api.js';
import StatsSummary from './stats/StatsSummary.jsx';
import StatsCharts from './stats/StatsCharts.jsx';
import StatsTopLists from './stats/StatsTopLists.jsx';
import PopularTab from './PopularTab.jsx';
import SubTabs from '../expressive/SubTabs.jsx';
import { useSubPage } from '../expressive/useSubPage.js';

/**
 * What the whole fleet has been listening to, and — one switch over — what everyone on the server
 * has. Both are listening charts, so they share a tab.
 *
 * Each player used to keep its own statistics — the desktop client from a local history file, the
 * phone from its own database — so neither ever showed a true total and the two disagreed with
 * each other by construction. Everything here is computed from one table that every device reports
 * into, which is the entire point of centralising it.
 */
const STATS_QUERY = `query Stats($user: String!, $period: String, $device: String) {
  listeningStats(userId: $user, period: $period, deviceName: $device) {
    secsToday secsTotal playsTotal streak firstPlayedAt
    topArtists { name value }
    topAlbums { name value }
    topTracks { name value }
    topGenres { name value }
    heatmap
    byHour
    byDevice { name value }
  }
}`;

const PERIODS = [
  { id: 'WEEK', label: 'This week' },
  { id: 'MONTH', label: 'This month' },
  { id: 'YEAR', label: 'This year' },
  { id: 'ALL', label: 'All time' }
];

const SCOPES = [
  { id: 'me', label: 'You', icon: User },
  { id: 'everyone', label: 'Everyone', icon: Users }
];

export default function StatsTab({ username, nodes = [], onUnauthorized }) {
  // `#/stats/everyone`, or the old `#/popular`, opens the server-wide chart.
  const [scope, setScope] = useSubPage('stats', ['me', 'everyone'], { popular: 'everyone' });
  const [period, setPeriod] = useState('MONTH');
  const [device, setDevice] = useState('');
  const [stats, setStats] = useState(null);
  // The dropdown's options, kept separately from `stats.byDevice`.
  //
  // They cannot come from the response being displayed: that response is filtered by the very
  // device selected, so it comes back listing only that one and the dropdown collapses to a single
  // choice — you could pick a device, but never a different one without going via "All devices"
  // first. Only an unfiltered response knows the whole fleet, so only an unfiltered response is
  // allowed to update this.
  const [deviceOptions, setDeviceOptions] = useState([]);

  const displayName = useCallback((rawName) => {
    const match = nodes.find(n => n.petname === rawName || n.deviceId === rawName);
    return match?.petname || rawName;
  }, [nodes]);

  const load = useCallback(async () => {
    try {
      const res = await gql(STATS_QUERY, { user: username, period, device: device || null });
      const body = await res.json();
      const next = body?.data?.listeningStats ?? null;
      setStats(next);
      if (!device && next) setDeviceOptions(next.byDevice.map(entry => entry.name));
    } catch (error) {
      if (error.unauthorized) onUnauthorized?.();
    }
  }, [username, period, device, onUnauthorized]);

  useEffect(() => {
    if (scope !== 'me') return undefined;
    load();
    // Statistics move slowly. A minute is frequent enough to feel live and rare enough not to
    // re-aggregate somebody's whole history every few seconds.
    const timer = setInterval(load, 60000);
    return () => clearInterval(timer);
  }, [load, scope]);

  const devices = Array.from(new Set([...nodes.map(n => n.petname).filter(Boolean), ...deviceOptions.map(displayName)]));
  const shown = stats && { ...stats, byDevice: stats.byDevice.map(d => ({ ...d, name: displayName(d.name) })) };

  return (
    <>
      <SubTabs pages={SCOPES} active={scope} onOpen={setScope}>
        {scope === 'me' && (
          <div className="stats-filters">
            <select className="pill-select pill-select-primary" value={period} onChange={e => setPeriod(e.target.value)} aria-label="Period">
              {PERIODS.map(p => <option key={p.id} value={p.id}>{p.label}</option>)}
            </select>
            <select className="pill-select" value={device} onChange={e => setDevice(e.target.value)} aria-label="Device">
              <option value="">All devices</option>
              {devices.map(name => <option key={name} value={name}>{name}</option>)}
            </select>
          </div>
        )}
      </SubTabs>

      {scope === 'everyone' && <PopularTab />}

      {scope === 'me' && !shown && <div className="empty-hint">Nothing played yet. Play something in Wanda or Wander.</div>}

      {scope === 'me' && shown && (
        <>
          <StatsSummary stats={shown} period={period} deviceName={device} />
          <StatsCharts heatmap={shown.heatmap} byHour={shown.byHour} />
          <StatsTopLists stats={shown} />
        </>
      )}
    </>
  );
}
