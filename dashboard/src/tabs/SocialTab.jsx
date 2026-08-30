import React, { useCallback, useEffect, useState } from 'react';
import {
  Activity,
  Award,
  Repeat,
  Sparkles,
  Trophy,
  Users,
  Music2,
  BarChart3
} from 'lucide-react';
import { gql } from '../api.js';
import Avatar from '../Avatar.jsx';

const SOCIAL_DATA_QUERY = `query SocialData($days: Int, $limit: Int, $period: String) {
  friends {
    profile {
      username
      displayName
      bio
      avatarUrl
      friendState
      showNowPlaying
      showStats
      showActivity
      discoverable
    }
    nowPlaying {
      username
      trackUri
      trackTitle
      artistName
      albumName
      artworkUrl
      positionMs
      isPlaying
      updatedAt
    }
  }
  friendActivity(days: $days, limit: $limit) {
    username
    at
    kind
    summary
    artist
    title
    count
  }
  circleRecap(period: $period) {
    period
    members
    anthem {
      title
      artist
      plays
      byMember {
        name
        value
      }
    }
    topTracks {
      name
      value
    }
    topArtists {
      name
      value
    }
    trendsetter {
      username
      firsts
      examples
    }
    matrix {
      a
      b
      score
    }
  }
}`;

const PERIODS = ['WEEK', 'MONTH', 'YEAR', 'ALL'];
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

export function formatWhen(value) {
  if (!value) return '';
  const then = new Date(value);
  if (Number.isNaN(then.getTime())) return value;
  const seconds = Math.max(0, (Date.now() - then.getTime()) / 1000);
  if (seconds < 60) return 'just now';
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h ago`;
  if (seconds < 7 * 86400) return `${Math.floor(seconds / 86400)}d ago`;
  return then.toLocaleDateString();
}

function periodNoun(period) {
  switch (period) {
    case 'WEEK': return 'week';
    case 'YEAR': return 'year';
    case 'ALL': return 'all time';
    default: return 'month';
  }
}

export default function SocialTab({ onUnauthorized }) {
  const [activeSection, setActiveSection] = useState('overview'); // 'overview' | 'feed' | 'recap'
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

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: '16px' }}>
      {/* Section Switcher */}
      <div className="card" style={{ padding: '8px 16px' }}>
        <div className="segmented">
          <button
            className={`segmented-btn ${activeSection === 'overview' ? 'active' : ''}`}
            onClick={() => setActiveSection('overview')}
          >
            <Users size={14} style={{ marginRight: '6px' }} />
            Friends ({friends.length})
          </button>
          <button
            className={`segmented-btn ${activeSection === 'feed' ? 'active' : ''}`}
            onClick={() => setActiveSection('feed')}
          >
            <Activity size={14} style={{ marginRight: '6px' }} />
            Activity Feed
          </button>
          <button
            className={`segmented-btn ${activeSection === 'recap' ? 'active' : ''}`}
            onClick={() => setActiveSection('recap')}
          >
            <BarChart3 size={14} style={{ marginRight: '6px' }} />
            Circle Stats & Recap
          </button>
        </div>
      </div>

      {/* 1. Friends & Presence Overview */}
      {activeSection === 'overview' && (
        <div style={{ display: 'flex', flexDirection: 'column', gap: '16px' }}>
          <div className="card">
            <div className="card-header">
              <div>
                <div className="card-title">Friends & Presence</div>
                <div className="card-subtitle">
                  Real-time status and listening activity across your connected circle
                </div>
              </div>
            </div>

            {friends.length === 0 ? (
              <div className="empty-hint" style={{ padding: '32px 16px', textAlign: 'center' }}>
                {loaded
                  ? 'No friends connected yet. Connect with other Agro users using friend codes or invites in the People tab.'
                  : 'Loading friends…'}
              </div>
            ) : (
              <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fill, minmax(280px, 1fr))', gap: '12px', marginTop: '8px' }}>
                {friends.map(({ profile, nowPlaying }) => {
                  const isPlaying = nowPlaying?.isPlaying;
                  return (
                    <div
                      key={profile.username}
                      className="card"
                      style={{
                        padding: '14px',
                        background: 'var(--bg-surface-elevated)',
                        borderColor: isPlaying ? 'var(--status-active-border)' : 'var(--border-subtle)',
                        display: 'flex',
                        flexDirection: 'column',
                        gap: '10px'
                      }}
                    >
                      <div style={{ display: 'flex', alignItems: 'center', gap: '12px' }}>
                        <Avatar username={profile.username} size={40} />
                        <div style={{ flex: 1, minWidth: 0 }}>
                          <div style={{ display: 'flex', alignItems: 'center', gap: '6px' }}>
                            <span style={{ fontWeight: '600', fontSize: '14px' }}>
                              {profile.displayName || profile.username}
                            </span>
                            {profile.displayName && (
                              <span style={{ fontSize: '11px', color: 'var(--text-muted)' }}>
                                @{profile.username}
                              </span>
                            )}
                          </div>
                          {profile.bio && (
                            <div style={{ fontSize: '12px', color: 'var(--text-muted)', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                              {profile.bio}
                            </div>
                          )}
                        </div>
                      </div>

                      {/* Live Now Playing status */}
                      {isPlaying ? (
                        <div
                          style={{
                            background: 'var(--status-active-bg)',
                            border: '1px solid var(--status-active-border)',
                            borderRadius: 'var(--radius-sm)',
                            padding: '8px 10px',
                            display: 'flex',
                            alignItems: 'center',
                            gap: '10px'
                          }}
                        >
                          <div className="audio-pulse-indicator is-playing">
                            <span className="wave-bar bar-1" />
                            <span className="wave-bar bar-2" />
                            <span className="wave-bar bar-3" />
                            <span className="wave-bar bar-4" />
                          </div>
                          <div style={{ flex: 1, minWidth: 0 }}>
                            <div style={{ fontSize: '12px', fontWeight: '600', color: 'var(--status-active)', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                              {nowPlaying.trackTitle}
                            </div>
                            <div style={{ fontSize: '11px', color: 'var(--text-secondary)', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                              {nowPlaying.artistName}
                            </div>
                          </div>
                        </div>
                      ) : (
                        <div style={{ fontSize: '11px', color: 'var(--text-muted)', display: 'flex', alignItems: 'center', gap: '6px' }}>
                          <span style={{ width: '6px', height: '6px', borderRadius: '50%', background: 'var(--border-strong)' }} />
                          {profile.showNowPlaying ? 'Not playing right now' : 'Presence hidden by user'}
                        </div>
                      )}

                      {/* Privacy / Feature Badges */}
                      <div style={{ display: 'flex', gap: '6px', flexWrap: 'wrap', marginTop: 'auto', paddingTop: '4px' }}>
                        {profile.showStats && (
                          <span style={{ fontSize: '10px', padding: '1px 6px', borderRadius: '4px', background: 'var(--bg-surface)', border: '1px solid var(--border-subtle)', color: 'var(--text-secondary)' }}>
                            Stats shared
                          </span>
                        )}
                        {profile.showActivity && (
                          <span style={{ fontSize: '10px', padding: '1px 6px', borderRadius: '4px', background: 'var(--bg-surface)', border: '1px solid var(--border-subtle)', color: 'var(--text-secondary)' }}>
                            Activity shared
                          </span>
                        )}
                      </div>
                    </div>
                  );
                })}
              </div>
            )}
          </div>
        </div>
      )}

      {/* 2. Activity Feed */}
      {activeSection === 'feed' && (
        <div className="card">
          <div className="card-header">
            <div>
              <div className="card-title">Friend Activity</div>
              <div className="card-subtitle">
                Milestones, repeat plays, and new favourites across your circle
              </div>
            </div>
            <div className="segmented">
              {WINDOWS.map(w => (
                <button
                  key={w.days}
                  className={`segmented-btn ${days === w.days ? 'active' : ''}`}
                  onClick={() => setDays(w.days)}
                >
                  {w.label}
                </button>
              ))}
            </div>
          </div>

          {feedItems.length === 0 ? (
            <div className="empty-hint" style={{ padding: '36px 16px', textAlign: 'center' }}>
              <div style={{ fontSize: '14px', fontWeight: '500', marginBottom: '6px', color: 'var(--text-secondary)' }}>
                {loaded ? 'No recent activity events' : 'Loading activity…'}
              </div>
              <p style={{ fontSize: '12px', color: 'var(--text-muted)', maxWidth: '460px', margin: '0 auto' }}>
                Activity milestones are automatically generated when friends with <em>Show activity</em> enabled reach listening thresholds (e.g. 10+ plays of an artist, 4+ plays of a track within 24 hours, or 5+ tracks from a newly discovered artist).
              </p>
            </div>
          ) : (
            <div className="feed-list">
              {feedItems.map((item, index) => {
                const Icon = ICONS[item.kind] ?? Activity;
                return (
                  <div
                    key={`${item.username}-${item.at}-${index}`}
                    className="feed-item"
                    style={{ display: 'flex', alignItems: 'center', gap: '14px', padding: '12px 14px' }}
                  >
                    <Avatar username={item.username} size={36} />
                    <div className="feed-body" style={{ flex: 1 }}>
                      <div className="feed-summary" style={{ display: 'flex', alignItems: 'center', gap: '6px' }}>
                        <Icon size={14} color="var(--status-active)" />
                        <span>
                          <strong>@{item.username}</strong> {item.summary || `listened to ${item.title}`}
                        </span>
                      </div>
                      <div className="feed-meta" style={{ marginTop: '3px' }}>
                        {item.artist ? `${item.artist} · ` : ''}{formatWhen(item.at)}
                      </div>
                    </div>
                  </div>
                );
              })}
            </div>
          )}
        </div>
      )}

      {/* 3. Circle Recap & Stats */}
      {activeSection === 'recap' && (
        <div style={{ display: 'flex', flexDirection: 'column', gap: '16px' }}>
          <div className="card">
            <div className="card-header">
              <div>
                <div className="card-title">Circle Recap &amp; Taste</div>
                <div className="card-subtitle">
                  Aggregated statistics across you and friends who share their listening stats
                </div>
              </div>
              <div className="segmented">
                {PERIODS.map(opt => (
                  <button
                    key={opt}
                    className={`segmented-btn ${period === opt ? 'active' : ''}`}
                    onClick={() => setPeriod(opt)}
                  >
                    {opt[0] + opt.slice(1).toLowerCase()}
                  </button>
                ))}
              </div>
            </div>

            {!recap ? (
              <div className="empty-hint" style={{ padding: '36px 16px', textAlign: 'center' }}>
                {loaded ? 'No circle recap data available for this period.' : 'Reading recap…'}
              </div>
            ) : (
              <div style={{ display: 'flex', flexDirection: 'column', gap: '16px' }}>
                {/* Circle Members Header */}
                <div className="top-list">
                  <div className="top-row" style={{ display: 'flex', alignItems: 'center', gap: '8px', flexWrap: 'wrap' }}>
                    <Users size={14} />
                    <span className="top-name" style={{ display: 'inline-flex', alignItems: 'center', gap: '8px', flexWrap: 'wrap' }}>
                      {recap.members.map(m => (
                        <span key={m} style={{ display: 'inline-flex', alignItems: 'center', gap: '4px' }}>
                          <Avatar username={m} size={20} /> @{m}
                        </span>
                      ))}
                    </span>
                    <span className="top-value">
                      {recap.members.length} {recap.members.length === 1 ? 'member' : 'members'}
                    </span>
                  </div>
                </div>

                {/* Anthem of the Period */}
                {recap.anthem && (
                  <div className="recap-anthem" style={{ padding: '16px', borderRadius: 'var(--radius-md)', background: 'var(--bg-surface-elevated)', border: '1px solid var(--border-subtle)' }}>
                    <div className="recap-label" style={{ fontSize: '11px', textTransform: 'uppercase', letterSpacing: '0.05em', color: 'var(--text-muted)', marginBottom: '4px' }}>
                      Anthem of the {periodNoun(recap.period)}
                    </div>
                    <div className="recap-anthem-title" style={{ fontSize: '16px', fontWeight: '700' }}>
                      {recap.anthem.title}
                    </div>
                    <div className="recap-anthem-artist" style={{ fontSize: '13px', color: 'var(--text-secondary)', marginBottom: '8px' }}>
                      {recap.anthem.artist}
                    </div>
                    <div className="feed-meta">
                      {recap.anthem.plays} total plays ·{' '}
                      {recap.anthem.byMember.map(entry => `@${entry.name} (${entry.value})`).join(' · ')}
                    </div>
                  </div>
                )}

                {/* Trendsetter */}
                {recap.trendsetter && (
                  <div className="recap-anthem" style={{ padding: '16px', borderRadius: 'var(--radius-md)', background: 'var(--bg-surface-elevated)', border: '1px solid var(--border-subtle)' }}>
                    <div className="recap-label" style={{ fontSize: '11px', textTransform: 'uppercase', letterSpacing: '0.05em', color: 'var(--status-active)', display: 'flex', alignItems: 'center', gap: '4px', marginBottom: '6px' }}>
                      <Trophy size={13} /> Trendsetter
                    </div>
                    <div style={{ display: 'flex', alignItems: 'center', gap: '10px' }}>
                      <Avatar username={recap.trendsetter.username} size={28} />
                      <span style={{ fontWeight: '600', fontSize: '14px' }}>@{recap.trendsetter.username}</span>
                    </div>
                    <div className="feed-meta" style={{ marginTop: '6px' }}>
                      First to listen to {recap.trendsetter.firsts} of the circle&apos;s top tracks
                      {recap.trendsetter.examples.length > 0 &&
                        ` (${recap.trendsetter.examples.join(', ')})`}
                    </div>
                  </div>
                )}

                {/* Top tracks & artists */}
                <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: '12px' }}>
                  <TopListCard title="Circle's Top Tracks" entries={recap.topTracks} icon={Music2} />
                  <TopListCard title="Circle's Top Artists" entries={recap.topArtists} icon={Users} />
                </div>

                {/* Taste Match Matrix */}
                <div className="card" style={{ background: 'var(--bg-surface-elevated)', padding: '14px' }}>
                  <div className="card-header" style={{ marginBottom: '10px' }}>
                    <div className="card-title" style={{ fontSize: '14px' }}>Taste Match Matrix</div>
                  </div>
                  {recap.matrix.length === 0 ? (
                    <div className="empty-hint">Nobody with overlapping history to compare yet.</div>
                  ) : (
                    <div className="top-list">
                      {recap.matrix.map(entry => (
                        <div key={`${entry.a}-${entry.b}`} className="top-row">
                          <span className="top-name">
                            @{entry.a} &amp; @{entry.b}
                          </span>
                          <span className="top-value" style={{ color: 'var(--status-active)', fontWeight: '600' }}>
                            {entry.score}%
                          </span>
                        </div>
                      ))}
                    </div>
                  )}
                </div>
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

function TopListCard({ title, entries, icon: Icon }) {
  return (
    <div className="card" style={{ background: 'var(--bg-surface-elevated)', padding: '14px' }}>
      <div className="card-header" style={{ marginBottom: '8px' }}>
        <div className="card-title" style={{ fontSize: '13px', display: 'flex', alignItems: 'center', gap: '6px' }}>
          {Icon && <Icon size={14} color="var(--text-muted)" />}
          {title}
        </div>
      </div>
      {(!entries || entries.length === 0) ? (
        <div className="empty-hint" style={{ padding: '16px 0' }}>Nothing recorded for this period.</div>
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
