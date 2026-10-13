import { BarChart3, Radio, Search, Sparkles, TrendingUp } from 'lucide-react';

/** Every visibility switch, each one sentence. They all start off; see the README's privacy model. */
const SWITCHES = [
  { key: 'showNowPlaying', icon: Radio, title: 'Now playing', line: 'Friends see what you play.' },
  { key: 'showStats', icon: BarChart3, title: 'Circle stats', line: 'You count in recaps.' },
  { key: 'showActivity', icon: Sparkles, title: 'Milestones', line: 'Friends see your streaks.' },
  { key: 'discoverable', icon: Search, title: 'Findable', line: 'You appear in search.' },
  { key: 'popularOptIn', icon: TrendingUp, title: 'Everyone chart', line: 'Your plays count, unnamed.' }
];

export default function PrivacyTiles({ visibility, onChange }) {
  return (
    <div className="privacy-tiles">
      {SWITCHES.map(({ key, icon: Icon, title, line }) => (
        <label key={key} className={`privacy-tile ${visibility[key] ? 'is-on' : ''}`}>
          <span className="privacy-icon"><Icon size={20} /></span>
          <span className="privacy-text">
            <span className="privacy-title">{title}</span>
            <span className="privacy-line">{line}</span>
          </span>
          <span className="m3-switch">
            <input
              type="checkbox"
              checked={!!visibility[key]}
              onChange={e => onChange({ ...visibility, [key]: e.target.checked })}
            />
            <span className="m3-switch-track"><span className="m3-switch-thumb" /></span>
          </span>
        </label>
      ))}
    </div>
  );
}
