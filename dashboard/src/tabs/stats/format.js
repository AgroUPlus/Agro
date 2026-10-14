/** Helpers shared by the Stats tab's pieces. */

export function formatHours(seconds) {
  if (!seconds) return '0m';
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.round((seconds % 3600) / 60);
  return hours > 0 ? `${hours}h ${minutes}m` : `${minutes}m`;
}

/** Hours only, for a figure that has to stay short on a tile. */
export function formatHoursShort(seconds) {
  if (!seconds) return '0h';
  const hours = seconds / 3600;
  return hours >= 10 ? `${Math.round(hours)}h` : `${hours.toFixed(1).replace(/\.0$/, '')}h`;
}

/** The server's names for a top album or track carry the artist as "Title — Artist". */
export function splitTitle(name) {
  const at = name.lastIndexOf(' — ');
  return at === -1 ? { title: name, artist: '' } : { title: name.slice(0, at), artist: name.slice(at + 3) };
}

const PERIOD_DAYS = { WEEK: 7, MONTH: 30, YEAR: 365 };

/**
 * How many days the period spans, to average over. ALL is as long as the history actually is,
 * which is why the server reports when the first play in it happened.
 */
export function periodDays(period, firstPlayedAt) {
  if (PERIOD_DAYS[period]) return PERIOD_DAYS[period];
  if (!firstPlayedAt) return 1;
  return Math.max(1, Math.ceil((Date.now() / 1000 - firstPlayedAt) / 86400));
}

/**
 * The server buckets hours in UTC. Rotated onto this browser's clock when the offset is a whole
 * number of hours; a half-hour zone keeps UTC, and says so, rather than smearing every bucket.
 */
export function hoursOnLocalClock(byHour) {
  const offsetMinutes = -new Date().getTimezoneOffset();
  if (offsetMinutes % 60 !== 0) return { values: byHour, local: false };
  const shift = offsetMinutes / 60;
  const values = byHour.map((_, localHour) => byHour[(((localHour - shift) % 24) + 24) % 24]);
  return { values, local: true };
}
