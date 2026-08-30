import React from 'react';
import { Smartphone, Terminal, Radio } from 'lucide-react';
import { formatDuration } from '../api.js';

export default function NowBar({ lastHandoff, nodes = [] }) {
  const isPlaying = !!lastHandoff?.isPlaying;
  const title = lastHandoff?.title || 'No active playback';
  const artist = lastHandoff?.artist || '';
  const album = lastHandoff?.album || '';
  const artworkUrl = lastHandoff?.artworkUrl || '';
  const positionSec = Math.floor((lastHandoff?.positionMs || 0) / 1000);
  const durationSec = Math.floor((lastHandoff?.durationMs || 0) / 1000);
  const progressPercent = durationSec > 0 ? Math.min(100, (positionSec / durationSec) * 100) : 0;

  const activeNode = nodes.find((n) => n.deviceId === lastHandoff?.deviceId);
  const devicePetname = activeNode?.petname || lastHandoff?.deviceId || 'fleet';
  const isMobile = activeNode?.clientType?.toLowerCase().includes('wanda');

  return (
    <footer className="modern-now-bar">
      {/* Left: Animated Pulse/Wave + Cover + Track details */}
      <div className="now-bar-media">
        <div className={`audio-pulse-indicator ${isPlaying ? 'is-playing' : ''}`}>
          <span className="wave-bar bar-1" />
          <span className="wave-bar bar-2" />
          <span className="wave-bar bar-3" />
          <span className="wave-bar bar-4" />
        </div>

        {artworkUrl ? (
          <img src={artworkUrl} alt={title} className="now-bar-art" />
        ) : (
          <div className="now-bar-art-placeholder">
            <Radio size={16} />
          </div>
        )}

        <div className="now-bar-info">
          <div className="now-bar-track-title">{title}</div>
          <div className="now-bar-subtext">
            {artist ? <span className="now-bar-artist">{artist}</span> : null}
            {album ? <span className="now-bar-album">{album}</span> : null}
          </div>
        </div>
      </div>

      {/* Middle: Progress scrub bar & elapsed / duration */}
      <div className="now-bar-center">
        <div className="now-bar-progress-container">
          <span className="time-display">{formatDuration(positionSec)}</span>
          <div className="now-bar-progress-track">
            <div
              className="now-bar-progress-fill"
              style={{ width: `${progressPercent}%` }}
            />
          </div>
          <span className="time-display total">
            {durationSec > 0 ? formatDuration(durationSec) : '--:--'}
          </span>
        </div>
      </div>

      {/* Right: Device & status badge */}
      <div className="now-bar-meta-right">
        {isPlaying && (
          <span className="quality-pill">LOSSLESS</span>
        )}
        <div className="device-indicator-pill">
          {isMobile ? <Smartphone size={12} /> : <Terminal size={12} />}
          <span>{devicePetname}</span>
          <span className={`live-dot ${isPlaying ? 'active' : ''}`} />
        </div>
      </div>
    </footer>
  );
}
