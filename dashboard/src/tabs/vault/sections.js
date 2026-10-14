import { History, KeyRound, Library, Merge, Podcast, Settings, Package } from 'lucide-react';

/** What each section the app sends is called and drawn as. An unknown one keeps its own name. */
const SECTIONS = {
  SETTINGS: { label: 'Settings', icon: Settings },
  ACCOUNTS: { label: 'Sign-ins', icon: KeyRound },
  HISTORY: { label: 'History', icon: History },
  LIBRARY: { label: 'Library', icon: Library },
  MERGES: { label: 'Merges', icon: Merge },
  EPISODES: { label: 'Podcasts', icon: Podcast }
};

export function sectionMeta(name) {
  return SECTIONS[name] ?? { label: name.charAt(0) + name.slice(1).toLowerCase(), icon: Package };
}

export function ago(iso) {
  const seconds = Math.max(0, (Date.now() - new Date(iso).getTime()) / 1000);
  const units = [['day', 86400], ['hour', 3600], ['minute', 60]];
  for (const [unit, size] of units) {
    const n = Math.floor(seconds / size);
    if (n >= 1) return `${n} ${unit}${n === 1 ? '' : 's'} ago`;
  }
  return 'just now';
}
