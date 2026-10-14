/** Everything the Social tab shows, in one round trip. */
export const SOCIAL_DATA_QUERY = `query SocialData($days: Int, $limit: Int, $period: String) {
  friends {
    profile { username displayName bio avatarUrl friendState showNowPlaying showStats showActivity }
    nowPlaying { trackTitle artistName albumName artworkUrl isPlaying updatedAt }
  }
  friendActivity(days: $days, limit: $limit) { username at kind summary artist title count }
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

export function periodNoun(period) {
  switch (period) {
    case 'WEEK': return 'week';
    case 'YEAR': return 'year';
    case 'ALL': return 'all time';
    default: return 'month';
  }
}
