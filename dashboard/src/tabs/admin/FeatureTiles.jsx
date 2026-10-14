import {
  Archive, AudioWaveform, Blend, Disc3, Gift, Headphones, Layers, Link2, Music2, Radio,
  ShieldCheck, Smartphone, Terminal, TrendingUp, Upload, Users
} from 'lucide-react';

/** What each server feature and connector is drawn with. Unknown ids get a generic icon. */
const ICONS = {
  'audio-relay': Radio,
  'library-uploads': Upload,
  'privacy-relay': ShieldCheck,
  'ephemeral-share': Link2,
  'friend-presence': Users,
  'listen-along': Headphones,
  jams: Disc3,
  'acoustic-search': AudioWaveform,
  'agro-wrapped': Gift,
  'popular-charts': TrendingUp,
  blends: Blend,
  'cloud-backups': Archive,
  'wander-tui': Terminal,
  'wanda-android': Smartphone,
  'subsonic-navidrome': Music2,
  'lrclib-lyrics': Music2
};

const iconFor = id => ICONS[id] ?? Layers;

/** One switchable feature: what it is, what it costs, and its switch. Dimmed while it is off. */
export function FeatureTile({ feature, busy, onToggle }) {
  const Icon = iconFor(feature.id);
  const cost = feature.metadata.find(m => m.key === 'Cost')?.value;
  return (
    <div className={`feature-tile ${feature.isEnabled ? 'is-on' : 'is-off'}`}>
      <div className="feature-top">
        <span className="feature-icon"><Icon size={22} /></span>
        <label className="switch">
          <input
            type="checkbox"
            aria-label={`${feature.name}: ${feature.isEnabled ? 'on' : 'off'}`}
            checked={feature.isEnabled}
            disabled={busy}
            onChange={() => onToggle(feature)}
          />
          <span className="slider" />
        </label>
      </div>
      <span className="feature-name">{feature.name}</span>
      <span className="feature-desc">{feature.description}</span>
      {cost && <span className="feature-cost" title={cost}>{cost}</span>}
    </div>
  );
}

/** A client or connector: status only, since there is nothing to switch off. */
export function ConnectorRow({ plugin }) {
  const Icon = iconFor(plugin.id);
  return (
    <li className="connector-row">
      <span className={`connector-icon ${plugin.isConnected ? 'is-on' : ''}`}><Icon size={20} /></span>
      <span className="connector-text">
        <span className="connector-name">{plugin.name}</span>
        <span className="connector-meta">
          {plugin.metadata.map(m => <span key={m.key}>{m.key}: <b>{m.value}</b></span>)}
        </span>
      </span>
      <span className={`connector-state ${plugin.isConnected ? 'is-on' : ''}`}>
        {plugin.isConnected ? 'Connected' : 'Not connected'}
      </span>
    </li>
  );
}
