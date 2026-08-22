import { useCallback, useEffect, useState } from 'react';
import { Activity, Award, Repeat, Sparkles, Trophy, Users } from 'lucide-react';
import { gql } from '../App.jsx';
import Avatar from '../Avatar.jsx';

/**
 * What your friends have been into, and what the circle looks like together.
 *
 * Both halves are derived on the server from the same plays the Stats tab aggregates — nothing here
 * has storage of its own. That is why a friend who closes a switch disappears from this page
 * immediately rather than leaving a stale copy behind.
 *
 * The two halves are gated separately and deliberately: the feed needs `showActivity` (a timeline),
 * the recap needs `showStats` (an aggregate). A friend can quite reasonably appear in one and not
 * the other, so neither section treats the other's emptiness as an error.
 */
const FEED_QUERY = `query Feed($days: Int, $limit: Int) {
  friendActivity(days: $days, limit: $limit) {
    username at kind summary artist title count
  }
}`;

const RECAP_QUERY = `query Recap($period: String) {
  circleRecap(period: $period) {
    period
    members
    anthem { title artist plays byMember { name value } }
    topTracks { name value }
    topArtists { name value }
    trendsetter { username firsts examples }
    matrix { a b score }
  }
}`;

const PERIODS = ['WEEK', 'MONTH', 'YEAR', 'ALL'];

/** How far back the feed looks, as the segmented control offers it. */
const WINDOWS = [
  { label: '7 days', days: 7 },
  { label: '14 days', days: 14 },
  { label: '30 days', days: 30 }
];

const ICONS = {
  MILESTONE: Award,
  ON_REPEAT: Repeat,
  NEW_FAVOURITE: Sparkles
};

export default function FeedTab({ onUnauthorized }) {
  const [days, setDays] = useState(14);
  const [period, setPeriod] = useState('MONTH');
  const [items, setItems] = useState([]);
  const [recap, setRecap] = useState(null);
  const [loaded, setLoaded] = useState(false);

  const load = useCallback(async () => {
    try {
      const [feedRes, recapRes] = await Promise.all([
        gql(FEED_QUERY, { days, limit: 60 }),
        gql(RECAP_QUERY, { period })
      ]);
      const feedBody = await feedRes.json();
      const recapBody = await recapRes.json();
      setItems(feedBody?.data?.friendActivity ?? []);
      setRecap(recapBody?.data?.circleRecap ?? null);
    } catch (error) {
      if (error.unauthorized) onUnauthorized?.();
    } finally {
      setLoaded(true);
    }
  }, [days, period, onUnauthorized]);

  useEffect(() => {
    load();
    // The same cadence as the Stats tab, for the same reason: this is an aggregate over history,
    // and re-deriving everybody's takes longer than it is worth doing often.
    const timer = setInterval(load, 60000);
    return () => clearInterval(timer);
  }, [load]);

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: '16px' }}>
      <div className="card">
        <div className="card-header">
          <div>
            <div className="card-title">Friend activity</div>
            <div className="card-subtitle">
              Only friends who have turned on <strong>Show activity</strong> appear here
            </div>
          </div>
          <div className="segmented">
            {WINDOWS.map(window => (
              <button
                key={window.days}
                className={`segmented-btn${days === window.days ? ' active' : ''}`}
                onClick={() => setDays(window.days)}
              >
                {window.label}
              </button>
            ))}
          </div>
        </div>

        {items.length === 0 ? (
          <div className="empty-hint">
            {loaded
              ? 'Nothing yet. Either nobody has opened their activity, or there has been nothing worth reporting.'
              : 'Reading…'}
          </div>
        ) : (
          <div className="feed-list">
            {items.map((item, index) => {
              const Icon = ICONS[item.kind] ?? Activity;
              return (
                <div key={`${item.username}-${item.at}-${index}`} className="feed-item" style={{ display: 'flex', alignItems: 'center', gap: '12px' }}>
                  <Avatar username={item.username} size={32} />
                  <div className="feed-body" style={{ flex: 1 }}>
                    <div className="feed-summary">{item.summary}</div>
                    <div className="feed-meta">{formatWhen(item.at)}</div>
                  </div>
                </div>
              );
            })}
          </div>
        )}
      </div>

      <div className="card">
        <div className="card-header">
          <div>
            <div className="card-title">Circle recap</div>
            <div className="card-subtitle">
              You, plus every friend who has opened their statistics
            </div>
          </div>
          <div className="segmented">
            {PERIODS.map(option => (
              <button
                key={option}
                className={`segmented-btn${period === option ? ' active' : ''}`}
                onClick={() => setPeriod(option)}
              >
                {option[0] + option.slice(1).toLowerCase()}
              </button>
            ))}
          </div>
        </div>

        {!recap ? (
          <div className="empty-hint">{loaded ? 'No recap to show.' : 'Reading…'}</div>
        ) : (
          <>
            <div className="top-list">
              <div className="top-row" style={{ display: 'flex', alignItems: 'center', gap: '8px', flexWrap: 'wrap' }}>
                <Users size={13} />
                <span className="top-name" style={{ display: 'inline-flex', alignItems: 'center', gap: '8px', flexWrap: 'wrap' }}>
                  {recap.members.map(m => (
                    <span key={m} style={{ display: 'inline-flex', alignItems: 'center', gap: '4px' }}>
                      <Avatar username={m} size={18} /> {m}
                    </span>
                  ))}
                </span>
                <span className="top-value">
                  {recap.members.length} {recap.members.length === 1 ? 'member' : 'members'}
                </span>
              </div>
            </div>

            {recap.anthem && (
              <div className="recap-anthem">
                <div className="recap-label">Anthem of the {periodNoun(recap.period)}</div>
                <div className="recap-anthem-title">{recap.anthem.title}</div>
                <div className="recap-anthem-artist">{recap.anthem.artist}</div>
                <div className="feed-meta">
                  {recap.anthem.plays} plays ·{' '}
                  {recap.anthem.byMember.map(entry => `${entry.name} ${entry.value}`).join(' · ')}
                </div>
              </div>
            )}

            {recap.trendsetter && (
              <div className="recap-anthem">
                <div className="recap-label">
                  <Trophy size={13} /> Trendsetter
                </div>
                <div style={{ display: 'flex', alignItems: 'center', gap: '8px', marginTop: '4px' }}>
                  <Avatar username={recap.trendsetter.username} size={26} />
                  <div className="recap-anthem-title">{recap.trendsetter.username}</div>
                </div>
                <div className="feed-meta">
                  First to {recap.trendsetter.firsts} of the circle&apos;s top tracks
                  {recap.trendsetter.examples.length > 0 &&
                    ` — ${recap.trendsetter.examples.join(', ')}`}
                </div>
              </div>
            )}

            <div className="stats-columns">
              <TopList title="The circle's tracks" entries={recap.topTracks} />
              <TopList title="The circle's artists" entries={recap.topArtists} />
              <div className="card">
                <div className="card-header">
                  <div className="card-title">Taste match</div>
                </div>
                {recap.matrix.length === 0 ? (
                  <div className="empty-hint">Nobody to compare with yet.</div>
                ) : (
                  <div className="top-list">
                    {recap.matrix.map(entry => (
                      <div key={`${entry.a}-${entry.b}`} className="top-row">
                        <span className="top-name">
                          {entry.a} &amp; {entry.b}
                        </span>
                        <span className="top-value">{entry.score}%</span>
                      </div>
                    ))}
                  </div>
                )}
              </div>
            </div>
          </>
        )}
      </div>
    </div>
  );
}

function TopList({ title, entries }) {
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
              <span className="top-name">{entry.name}</span>
              <span className="top-value">{entry.value}</span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function periodNoun(period) {
  switch (period) {
    case 'WEEK':
      return 'week';
    case 'YEAR':
      return 'year';
    case 'ALL':
      return 'time';
    default:
      return 'month';
  }
}

/**
 * A relative time, because the exact second a milestone landed is never the interesting part.
 *
 * Falls back to the raw string rather than rendering "Invalid Date": a timestamp we cannot parse is
 * still better shown than replaced with a lie.
 */
export function formatWhen(value) {
  const then = new Date(value);
  if (Number.isNaN(then.getTime())) return value;
  const seconds = Math.max(0, (Date.now() - then.getTime()) / 1000);
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h ago`;
  if (seconds < 7 * 86400) return `${Math.floor(seconds / 86400)}d ago`;
  return then.toLocaleDateString();
}
