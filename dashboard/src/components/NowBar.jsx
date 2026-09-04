import React, { useEffect, useRef } from 'react';
import { Smartphone, Terminal, Radio } from 'lucide-react';
import { formatDuration } from '../api.js';

export default function NowBar({ lastHandoff, nodes = [] }) {
  const isPlaying = !!lastHandoff?.isPlaying;
  // A sealed handoff carries only an envelope this dashboard cannot open — the plaintext fields
  // are a placeholder, so they are not shown.
  const isEncrypted = !!lastHandoff?.encryptedPayload;
  const title = isEncrypted
    ? 'Private Session (E2EE)'
    : (lastHandoff?.title || 'No active playback');
  const artist = isEncrypted ? '' : (lastHandoff?.artist || '');
  const album = lastHandoff?.album || '';
  const artworkUrl = lastHandoff?.artworkUrl || '';
  
  const basePositionMs = lastHandoff?.positionMs || 0;
  const durationMs = lastHandoff?.durationMs || 0;
  const durationSec = Math.floor(durationMs / 1000);

  const activeNode = nodes.find((n) => n.deviceId === lastHandoff?.deviceId);
  const devicePetname = activeNode?.petname || lastHandoff?.deviceId || 'fleet';
  const isMobile = activeNode?.clientType?.toLowerCase().includes('wanda');

  const fillRef = useRef(null);
  const timeRef = useRef(null);
  const rafRef = useRef(null);

  useEffect(() => {
    const startTime = Date.now();

    const updateDOM = (posMs) => {
      const posSec = Math.floor(posMs / 1000);
      if (timeRef.current) {
        const newTimeStr = formatDuration(posSec);
        if (timeRef.current.textContent !== newTimeStr) {
          timeRef.current.textContent = newTimeStr;
        }
      }
      if (fillRef.current) {
        if (durationMs > 0) {
          const pct = Math.min(100, (posMs / durationMs) * 100);
          fillRef.current.style.width = `${pct}%`;
        } else {
          fillRef.current.style.width = `0%`;
        }
      }
    };

    const tick = () => {
      if (!isPlaying) {
        updateDOM(basePositionMs);
        return;
      }
      const now = Date.now();
      const elapsed = now - startTime;
      const currentMs = Math.min(basePositionMs + elapsed, durationMs || Infinity);
      updateDOM(currentMs);
      rafRef.current = requestAnimationFrame(tick);
    };

    if (rafRef.current) cancelAnimationFrame(rafRef.current);
    if (isPlaying) {
      rafRef.current = requestAnimationFrame(tick);
    } else {
      updateDOM(basePositionMs);
    }

    return () => {
      if (rafRef.current) cancelAnimationFrame(rafRef.current);
    };
  }, [basePositionMs, durationMs, isPlaying]);

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
          <span className="time-display" ref={timeRef}>
            {formatDuration(Math.floor(basePositionMs / 1000))}
          </span>
          <div className="now-bar-progress-track">
            <div
              className="now-bar-progress-fill"
              ref={fillRef}
              style={{ width: durationMs > 0 ? `${Math.min(100, (basePositionMs / durationMs) * 100)}%` : '0%' }}
            />
          </div>
          <span className="time-display total">
            {durationSec > 0 ? formatDuration(durationSec) : '--:--'}
          </span>
        </div>
      </div>

      {/* Right: Device & status badge */}
      <div className="now-bar-meta-right">
        {isEncrypted && (
          <span className="quality-pill" style={{ background: '#3b82f6', color: '#fff' }}>E2EE</span>
        )}
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
