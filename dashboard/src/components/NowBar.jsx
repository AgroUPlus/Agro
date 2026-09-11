import React, { useEffect, useRef, useState } from 'react';
import { Smartphone, Terminal, Radio, Play, Pause } from 'lucide-react';
import { formatDuration } from '../api.js';

export default function NowBar({
  lastHandoff,
  nodes = [],
  onTogglePlay,
  onSeek,
  onSwitchDevice
}) {
  const isPlaying = lastHandoff?.isPlaying ?? true;
  const isEncrypted = !!lastHandoff?.encryptedPayload;
  const title = isEncrypted
    ? 'Private Session (E2EE)'
    : (lastHandoff?.title || 'No active playback');
  const artist = isEncrypted ? '' : (lastHandoff?.artist || '');
  const album = lastHandoff?.album || '';
  const artworkUrl = lastHandoff?.artworkUrl || '';

  const durationMs = lastHandoff?.durationMs || 212000;
  const durationSec = Math.floor(durationMs / 1000);
  const [currentMs, setCurrentMs] = useState(lastHandoff?.positionMs || 68000);

  const activeNode = nodes.find((n) => n.deviceId === lastHandoff?.deviceId) || nodes[0];
  const devicePetname = activeNode?.petname || lastHandoff?.deviceId || 'fleet';
  const isMobile = activeNode?.clientType?.toLowerCase().includes('wanda');

  const fillRef = useRef(null);
  const timeRef = useRef(null);
  const trackRef = useRef(null);
  const rafRef = useRef(null);

  // Sync when parent handoff position changes externally
  useEffect(() => {
    if (typeof lastHandoff?.positionMs === 'number') {
      setCurrentMs(lastHandoff.positionMs);
    }
  }, [lastHandoff?.positionMs]);

  useEffect(() => {
    let lastTime = Date.now();

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
        updateDOM(currentMs);
        return;
      }
      const now = Date.now();
      const delta = now - lastTime;
      lastTime = now;

      setCurrentMs((prev) => {
        const next = Math.min(prev + delta, durationMs || Infinity);
        updateDOM(next);
        return next;
      });

      rafRef.current = requestAnimationFrame(tick);
    };

    if (rafRef.current) cancelAnimationFrame(rafRef.current);
    if (isPlaying) {
      rafRef.current = requestAnimationFrame(tick);
    } else {
      updateDOM(currentMs);
    }

    return () => {
      if (rafRef.current) cancelAnimationFrame(rafRef.current);
    };
  }, [isPlaying, durationMs]);

  const handleTrackClick = (e) => {
    if (!trackRef.current || !durationMs) return;
    const rect = trackRef.current.getBoundingClientRect();
    const clickX = e.clientX - rect.left;
    const pct = Math.max(0, Math.min(1, clickX / rect.width));
    const newPos = Math.floor(pct * durationMs);
    setCurrentMs(newPos);
    if (onSeek) onSeek(newPos);
  };

  return (
    <footer className="modern-now-bar">
      {/* Left: Animated Pulse/Wave + Cover + Track details */}
      <div className="now-bar-media">
        <div className={`audio-pulse-indicator ${isPlaying ? 'is-playing' : 'is-paused'}`}>
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

      {/* Middle: Controls & Progress scrub bar */}
      <div className="now-bar-center">
        <div className="now-bar-controls">
          <button
            type="button"
            className="now-bar-play-btn"
            onClick={onTogglePlay}
            aria-label={isPlaying ? 'Pause' : 'Play'}
            title={isPlaying ? 'Pause' : 'Play'}
          >
            {isPlaying ? <Pause size={15} /> : <Play size={15} style={{ marginLeft: 2 }} />}
          </button>
        </div>

        <div className="now-bar-progress-container">
          <span className="time-display" ref={timeRef}>
            {formatDuration(Math.floor(currentMs / 1000))}
          </span>
          <div
            className="now-bar-progress-track interactive"
            ref={trackRef}
            onClick={handleTrackClick}
            title="Click to seek"
          >
            <div
              className="now-bar-progress-fill"
              ref={fillRef}
              style={{ width: durationMs > 0 ? `${Math.min(100, (currentMs / durationMs) * 100)}%` : '0%' }}
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
        <button
          type="button"
          className="device-indicator-pill interactive"
          onClick={onSwitchDevice}
          title="Click to switch active playback device"
        >
          {isMobile ? <Smartphone size={12} /> : <Terminal size={12} />}
          <span>{devicePetname}</span>
          <span className={`live-dot ${isPlaying ? 'active' : ''}`} />
        </button>
      </div>
    </footer>
  );
}
