/** `m:ss`, the way a track's length is read. */
export function formatClock(ms) {
  if (!ms) return '–';
  const total = Math.round(ms / 1000);
  return `${Math.floor(total / 60)}:${String(total % 60).padStart(2, '0')}`;
}

/** "about 1 hr 15 min", the way a whole playlist's length is read. */
export function formatTotal(ms) {
  const minutes = Math.round((ms || 0) / 60000);
  if (minutes < 60) return `${minutes} min`;
  const hours = Math.floor(minutes / 60);
  return `about ${hours} hr ${minutes % 60} min`;
}

export function formatDate(iso) {
  if (!iso) return '';
  const date = new Date(iso);
  return Number.isNaN(date.getTime())
    ? ''
    : date.toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' });
}

export const songCount = (n) => `${n} ${n === 1 ? 'song' : 'songs'}`;

export const VISIBILITY_LABEL = {
  PRIVATE: 'Private playlist',
  FRIENDS: 'Friends-only playlist',
  PUBLIC: 'Public playlist'
};
