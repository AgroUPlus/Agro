import { useCallback, useEffect, useState } from 'react';
import { Clock, Flame, Music2, Radio } from 'lucide-react';
import { gql } from '../App.jsx';

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

export default function StatsTab({ username, onUnauthorized }) {
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
    // Statistics move slowly. A minute is frequent enough to feel live and rare enough not to
    // re-aggregate somebody's whole history every few seconds.
    const timer = setInterval(load, 60000);
    return () => clearInterval(timer);
  }, [load]);

  if (!stats) {
    return (
      <div className="card">
        <div className="empty-hint">
          No listening recorded yet. Play something in <strong>wander</strong> or{' '}
          <strong>wanda</strong> and it will appear here.
        </div>
      </div>
    );
  }

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: '16px' }}>
      <div className="card">
        <div className="card-header">
          <div>
            <div className="card-title">Listening</div>
            <div className="card-subtitle">
              {device ? `${device} only` : 'Every device on this account'}
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
            {deviceOptions.map(name => (
              <option key={name} value={name}>
                {name}
              </option>
            ))}
          </select>
        </div>

        <div className="stat-tiles">
          <StatTile icon={<Clock size={15} />} label="Today" value={formatHours(stats.secsToday)} />
          <StatTile icon={<Clock size={15} />} label="This week" value={formatHours(stats.secsWeek)} />
          <StatTile icon={<Music2 size={15} />} label="Plays" value={stats.playsTotal} />
          <StatTile
            icon={<Flame size={15} />}
            label="Streak"
            value={`${stats.streak} ${stats.streak === 1 ? 'day' : 'days'}`}
          />
        </div>
      </div>

      <div className="card">
        <div className="card-header">
          <div className="card-title">Last 14 days</div>
        </div>
        <Bars values={stats.byDay} labelFor={index => `${stats.byDay.length - index - 1}d ago`} />
      </div>

      <div className="card">
        <div className="card-header">
          <div>
            <div className="card-title">By hour</div>
            <div className="card-subtitle">UTC, so it will be offset from your clock</div>
          </div>
        </div>
        <Bars values={stats.byHour} labelFor={index => `${String(index).padStart(2, '0')}:00`} />
      </div>

      <div className="stats-columns">
        <TopList title="Top artists" entries={stats.topArtists} unit="plays" />
        <TopList title="Top albums" entries={stats.topAlbums} unit="plays" />
        <TopList title="Top tracks" entries={stats.topTracks} unit="plays" />
      </div>

      <div className="card">
        <div className="card-header">
          <div className="card-title">By device</div>
        </div>
        {stats.byDevice.length === 0 ? (
          <div className="empty-hint">Nothing reported yet.</div>
        ) : (
          <div className="top-list">
            {stats.byDevice.map(entry => (
              <div key={entry.name} className="top-row">
                <Radio size={13} />
                <span className="top-name">{entry.name}</span>
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
          <div className="bar-fill" style={{ height: `${(value / peak) * 100}%` }} />
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
