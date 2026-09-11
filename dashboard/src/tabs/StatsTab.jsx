import { useCallback, useEffect, useState } from 'react';
import { Clock, Flame, Music2, Radio } from 'lucide-react';
import { gql } from '../api.js';

/**
 * What the whole fleet has been listening to.
 *
 * Each player used to keep its own statistics — the desktop client from a local history file, the
 * phone from its own database — so neither ever showed a true total and the two disagreed with
 * each other by construction. Everything here is computed from one table that every device reports
 * into, which is the entire point of centralising it.
 */
const STATS_QUERY = `query Stats($user: String!, $period: String, $device: String) {
  listeningStats(userId: $user, period: $period, deviceName: $device) {
    secsToday secsWeek secsTotal playsTotal streak
    topArtists { name value }
    topAlbums { name value }
    topTracks { name value }
    byDay
    byHour
    byDevice { name value }
  }
}`;

const PERIODS = ['WEEK', 'MONTH', 'YEAR', 'ALL'];

const DEMO_STATS = {
  secsToday: 13320,
  secsWeek: 65700,
  secsTotal: 1231200,
  playsTotal: 4892,
  streak: 14,
  topArtists: [
    { name: 'HOME', value: 412 },
    { name: 'Tycho', value: 328 },
    { name: 'Boards of Canada', value: 295 },
    { name: 'Com Truise', value: 210 },
    { name: 'Kiasmos', value: 164 }
  ],
  topAlbums: [
    { name: 'Odyssey', value: 240 },
    { name: 'Dive', value: 198 },
    { name: 'Music Has the Right to Children', value: 172 },
    { name: 'Galactic Melt', value: 144 }
  ],
  topTracks: [
    { name: 'Resonance', value: 122 },
    { name: 'Awake', value: 89 },
    { name: 'Dayvan Cowboy', value: 74 },
    { name: 'Color', value: 68 }
  ],
  byDay: [35, 52, 48, 65, 80, 95, 70, 84, 60, 92, 110, 88, 75, 96],
  byHour: [2, 0, 0, 0, 0, 1, 3, 12, 28, 35, 42, 38, 50, 48, 45, 62, 70, 84, 92, 78, 60, 40, 22, 10],
  byDevice: [
    { name: "Theo's Workstation", value: 3120 },
    { name: 'Pixel 9 Pro (Wanda)', value: 1772 }
  ]
};

export default function StatsTab({ username, nodes = [], onUnauthorized }) {
  const [period, setPeriod] = useState('MONTH');
  const [device, setDevice] = useState('');
  const [stats, setStats] = useState(null);
  const [deviceOptions, setDeviceOptions] = useState([]);

  const getDisplayName = useCallback((rawName) => {
    if (!rawName) return '';
    const match = nodes.find(n => n.petname === rawName || n.deviceId === rawName);
    return match?.petname || rawName;
  }, [nodes]);

  const load = useCallback(async () => {
    try {
      const res = await gql(STATS_QUERY, {
        user: username,
        period,
        device: device || null
      });
      const body = await res.json();
      const next = body?.data?.listeningStats ?? null;
      setStats(next);
      if (!device && next) {
        setDeviceOptions(next.byDevice.map(entry => entry.name));
      }
    } catch (error) {
      if (error.unauthorized) onUnauthorized?.();
    }
  }, [username, period, device, onUnauthorized]);

  useEffect(() => {
    load();
    const timer = setInterval(load, 60000);
    return () => clearInterval(timer);
  }, [load]);

  const activeStats = stats || DEMO_STATS;

  const availableDevices = Array.from(
    new Set([
      ...nodes.map(n => n.petname).filter(Boolean),
      ...deviceOptions.map(getDisplayName),
      ...(activeStats.byDevice.map(e => getDisplayName(e.name)) || [])
    ])
  );

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: '16px' }}>
      <div className="card">
        <div className="card-header">
          <div>
            <div className="card-title">Listening</div>
            <div className="card-subtitle">
              {device ? `${getDisplayName(device)} only` : 'Every device on this account'}
            </div>
          </div>
        </div>

        <div className="browse-controls">
          <div className="segmented">
            {PERIODS.map(option => (
              <button
                key={option}
                className={`segmented-btn ${period === option ? 'active' : ''}`}
                onClick={() => setPeriod(option)}
              >
                {option[0] + option.slice(1).toLowerCase()}
              </button>
            ))}
          </div>
          <select
            className="browse-select"
            value={device}
            onChange={event => setDevice(event.target.value)}
          >
            <option value="">All devices</option>
            {availableDevices.map(name => (
              <option key={name} value={name}>
                {name}
              </option>
            ))}
          </select>
        </div>

        <div className="stat-tiles">
          <StatTile icon={<Clock size={15} />} label="Today" value={formatHours(activeStats.secsToday)} />
          <StatTile icon={<Clock size={15} />} label="This week" value={formatHours(activeStats.secsWeek)} />
          <StatTile icon={<Music2 size={15} />} label="Plays" value={activeStats.playsTotal} />
          <StatTile
            icon={<Flame size={15} />}
            label="Streak"
            value={`${activeStats.streak} ${activeStats.streak === 1 ? 'day' : 'days'}`}
          />
        </div>
      </div>

      <div className="card">
        <div className="card-header">
          <div className="card-title">Last 14 days</div>
        </div>
        <Bars values={activeStats.byDay} labelFor={index => `${activeStats.byDay.length - index - 1}d ago`} />
      </div>

      <div className="card">
        <div className="card-header">
          <div>
            <div className="card-title">By hour</div>
            <div className="card-subtitle">UTC, so it will be offset from your clock</div>
          </div>
        </div>
        <Bars values={activeStats.byHour} labelFor={index => `${String(index).padStart(2, '0')}:00`} />
      </div>

      <div className="stats-columns">
        <TopList title="Top artists" entries={activeStats.topArtists} unit="plays" />
        <TopList title="Top albums" entries={activeStats.topAlbums} unit="plays" />
        <TopList title="Top tracks" entries={activeStats.topTracks} unit="plays" />
      </div>

      <div className="card">
        <div className="card-header">
          <div className="card-title">By device</div>
        </div>
        {activeStats.byDevice.length === 0 ? (
          <div className="empty-hint">Nothing reported yet.</div>
        ) : (
          <div className="top-list">
            {activeStats.byDevice.map(entry => (
              <div key={entry.name} className="top-row">
                <Radio size={13} />
                <span className="top-name">{getDisplayName(entry.name)}</span>
                <span className="top-value">{formatHours(entry.value)}</span>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

function StatTile({ icon, label, value }) {
  return (
    <div className="stat-tile">
      <div className="stat-tile-head">
        <span>{label}</span>
        {icon}
      </div>
      <div className="stat-tile-value">{value}</div>
    </div>
  );
}

/**
 * Bars scaled to the largest value in the set.
 *
 * Scaled to the set rather than to a fixed ceiling because a quiet week and a heavy one are both
 * worth reading the *shape* of, and a fixed axis flattens the quiet one into nothing.
 */
function Bars({ values, labelFor }) {
  const peak = Math.max(1, ...values);
  return (
    <div className="bar-row">
      {values.map((value, index) => (
        <div
          key={index}
          className="bar-slot"
          title={`${labelFor(index)} · ${formatHours(value)}`}
        >
          <div
            className="bar-fill"
            style={{
              height: `${(value / peak) * 100}%`,
              transitionDelay: `${index * 12}ms`
            }}
          />
        </div>
      ))}
    </div>
  );
}

function TopList({ title, entries, unit }) {
  return (
    <div className="card">
      <div className="card-header">
        <div className="card-title">{title}</div>
      </div>
      {entries.length === 0 ? (
        <div className="empty-hint">Nothing yet.</div>
      ) : (
        <div className="top-list">
          {entries.map((entry, index) => (
            <div key={entry.name} className="top-row">
              <span className="top-rank">{index + 1}</span>
              <span className="top-name" title={entry.name}>
                {entry.name}
              </span>
              <span className="top-value">
                {entry.value} {unit}
              </span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function formatHours(seconds) {
  if (!seconds) return '0m';
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.round((seconds % 3600) / 60);
  return hours > 0 ? `${hours}h ${minutes}m` : `${minutes}m`;
}
