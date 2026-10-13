import { Clock, Flame, Music2, Radio } from 'lucide-react';
import Cookie from '../../expressive/Cookie.jsx';
import { formatHours, formatHoursShort, periodDays, splitTitle } from './format.js';

const DEVICE_TONES = ['primary', 'tertiary', 'lemon', 'pink', 'outline'];

/**
 * The period in one glance, after Wanda's stats screen: one huge figure and the song the period was
 * mostly about, then two square tiles — total time on a tonal square, the daily average on a
 * scalloped "sunny" shape — and where the listening happened as one split bar.
 */
export default function StatsSummary({ stats, period, deviceName }) {
  const days = periodDays(period, stats.firstPlayedAt);
  const perDay = stats.playsTotal / days;
  const top = stats.topTracks[0] ? splitTitle(stats.topTracks[0].name) : null;

  return (
    <div className="stats-summary">
      <div className="card stats-hero">
        <div className="stats-big">
          <span className="stats-big-number">{stats.playsTotal.toLocaleString()}</span>
          <span className="stats-big-label">songs played</span>
        </div>

        {top && (
          <div className="stats-top-song">
            <Cookie shape="flower" size={64}>
              <Music2 size={26} />
            </Cookie>
            <div className="stats-top-song-text">
              <span className="stats-overline">Top song</span>
              <span className="stats-top-title">{top.title}</span>
              {top.artist && <span className="stats-top-artist">{top.artist}</span>}
            </div>
            <span className="stats-top-count">×{stats.topTracks[0].value}</span>
          </div>
        )}

        <div className="stats-facts">
          <span className="stats-fact"><Flame size={15} />{stats.streak} day streak</span>
          <span className="stats-fact"><Clock size={15} />{formatHours(stats.secsToday)} today</span>
          {stats.topGenres?.[0] && <span className="stats-fact"><Radio size={15} />Mostly {stats.topGenres[0].name}</span>}
        </div>

        {!deviceName && stats.byDevice.length > 0 && <DeviceSplit devices={stats.byDevice} />}
      </div>

      <div className="stats-tiles-square">
        <div className="stats-square stats-square-time">
          <Clock size={26} />
          <div>
            <span className="stats-square-value">{formatHoursShort(stats.secsTotal)}</span>
            <span className="stats-square-label">listened</span>
          </div>
        </div>
        <div className="stats-square stats-square-sunny">
          <Cookie shape="sunny" className="stats-sunny-shape" />
          <div className="stats-sunny-text">
            <span className="stats-square-value">{perDay >= 10 ? Math.round(perDay) : perDay.toFixed(1)}</span>
            <span className="stats-square-label">plays a day</span>
          </div>
        </div>
      </div>
    </div>
  );
}

/** Every device's share of the listening time as one bar, with a legend under it. */
function DeviceSplit({ devices }) {
  const total = devices.reduce((sum, d) => sum + d.value, 0) || 1;
  return (
    <div className="device-split">
      <div className="device-split-bar">
        {devices.map((device, index) => (
          <span
            key={device.name}
            className={`tone-${DEVICE_TONES[index % DEVICE_TONES.length]}`}
            style={{ flexGrow: device.value }}
            title={`${device.name} · ${formatHours(device.value)}`}
          />
        ))}
      </div>
      <div className="device-split-legend">
        {devices.map((device, index) => (
          <span key={device.name}>
            <i className={`tone-${DEVICE_TONES[index % DEVICE_TONES.length]}`} />
            {device.name}
            <b>{Math.round((device.value / total) * 100)}%</b>
          </span>
        ))}
      </div>
    </div>
  );
}
