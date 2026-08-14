import React, { useState, useEffect } from 'react';
import { QRCodeSVG } from 'qrcode.react';
import { 
  Radio, Smartphone, Terminal, Server, 
  Layers, KeyRound, ScrollText, Copy, 
  Check, RefreshCw, Disc, Sliders, Save,
  User, Users, ChevronDown, Plus
} from 'lucide-react';

const FALLBACK_RULES = [
  {
    id: 'auto-handoff',
    name: 'Spotify-Style Playback Hand-off',
    description: 'Relays active track URI, metadata, and millisecond timestamp across Wander (Desktop) and Wanda (Android) so resuming on any device is a 1-tap/key prompt.',
    target: 'Wanda ↔ Wander',
    isEnabled: true
  },
  {
    id: 'settings-sync',
    name: 'Cross-Device Settings Synchronizer',
    description: 'Automatically synchronizes shared Subsonic credentials, LRCLIB lyrics resolvers, audio quality, and plugin keys between desktop and mobile.',
    target: 'All Clients',
    isEnabled: true
  },
  {
    id: 'wifi-precache',
    name: 'Proactive Wi-Fi Smart Pre-Caching',
    description: 'Instructs Wanda to automatically pre-download the top 15 next queued tracks over unmetered Wi-Fi connections.',
    target: 'Wanda Mobile',
    isEnabled: true
  },
  {
    id: 'lrclib-lyrics-hub',
    name: 'Central LRCLIB Synced Lyrics Hub',
    description: 'Background LRCLIB resolver that fetches and caches synchronized LRC lyrics in SQLite for all connected devices with zero duplicate queries.',
    target: 'All Clients',
    isEnabled: true
  },
  {
    id: 'jam-session',
    name: 'Democratic Jam Session Synchronizer',
    description: 'Maintains collaborative voting queues and sub-millisecond playback clock synchronization for active shared room sessions.',
    target: 'Multi-device',
    isEnabled: true
  }
];

export default function App() {
  const [activeTab, setActiveTab] = useState('nodes');
  const [username, setUsername] = useState('alpha');
  const [usersList, setUsersList] = useState(['alpha']);
  const [showUserMenu, setShowUserMenu] = useState(false);
  const [newUsernameInput, setNewUsernameInput] = useState('');
  const [passphrase, setPassphrase] = useState('tempest-omega-pioneer-terra');
  const [copied, setCopied] = useState(false);
  const [rules, setRules] = useState(FALLBACK_RULES);
  const [nodes, setNodes] = useState([]);
  const [syncedSettings, setSyncedSettings] = useState({
    serverUrl: 'http://localhost:4533',
    serverUsername: 'alpha',
    lrclibUrl: 'https://lrclib.net',
    lyricsFetchOnline: true,
    jamendoClientId: '',
    streamFormat: 'FLAC'
  });
  const [settingsSaved, setSettingsSaved] = useState(false);

  const [lastHandoff, setLastHandoff] = useState({
    title: 'No active playback',
    artist: 'Idle',
    album: '',
    positionMs: 0,
    durationMs: 0,
    isPlaying: false,
    deviceId: 'None'
  });

  const [syncLogs, setSyncLogs] = useState([
    { time: new Date().toLocaleTimeString(), event: '[DAEMON] Agro background sync daemon active on port 8700' }
  ]);

  // Query live user passphrase, plugins, nodes, and handoff from Agro GraphQL backend
  useEffect(() => {
    async function loadBackendData() {
      try {
        const res = await fetch('/graphql', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({
            query: `
              query LoadInitialState {
                users
                me(username: "${username}") {
                  username
                  passphrase
                }
                plugins {
                  id
                  name
                  description
                  target
                  isEnabled
                }
                activeNodes(userId: "${username}") {
                  deviceId
                  petname
                  clientType
                  ipAddress
                  version
                  currentTrack
                  lastSeenAt
                  isOnline
                }
                playbackHandoff(userId: "${username}") {
                  trackTitle
                  artistName
                  albumName
                  positionMs
                  isPlaying
                  deviceId
                }
                syncedSettings(userId: "${username}") {
                  serverUrl
                  serverUsername
                  lrclibUrl
                  lyricsFetchOnline
                  streamFormat
                }
              }
            `
          })
        });

        if (res.ok) {
          const { data } = await res.json();
          if (data?.users && data.users.length > 0) {
            setUsersList(data.users);
          }
          if (data?.me) {
            setUsername(data.me.username);
            setPassphrase(data.me.passphrase);
          }
          if (data?.plugins && data.plugins.length > 0) {
            setRules(data.plugins.map(p => ({
              id: p.id,
              name: p.name,
              description: p.description,
              target: p.target,
              isEnabled: p.isEnabled
            })));
          }
          if (data?.activeNodes) {
            setNodes(data.activeNodes);
          }
          if (data?.syncedSettings) {
            setSyncedSettings(s => ({
              ...s,
              serverUrl: data.syncedSettings.serverUrl || s.serverUrl,
              serverUsername: data.syncedSettings.serverUsername || s.serverUsername,
              lrclibUrl: data.syncedSettings.lrclibUrl || s.lrclibUrl,
              lyricsFetchOnline: data.syncedSettings.lyricsFetchOnline ?? s.lyricsFetchOnline,
              jamendoClientId: data.syncedSettings.jamendoClientId || '',
              streamFormat: data.syncedSettings.streamFormat || 'FLAC'
            }));
          }
          if (data?.playbackHandoff && data.playbackHandoff.trackTitle) {
            setLastHandoff(prev => ({
              ...prev,
              title: data.playbackHandoff.trackTitle,
              artist: data.playbackHandoff.artistName,
              album: data.playbackHandoff.albumName || "Unknown Album",
              positionMs: data.playbackHandoff.positionMs,
              durationMs: 243000,
              isPlaying: data.playbackHandoff.isPlaying,
              deviceId: data.playbackHandoff.deviceId
            }));
          }
          setSyncLogs(logs => [
            { time: new Date().toLocaleTimeString(), event: '[GRAPHQL] Initialized dynamic nodes and state from SQLite' },
            ...logs
          ]);
        }
      } catch (e) {}
    }

    loadBackendData();

    // Polling every 2.5 seconds to refresh state & nodes
    const interval = setInterval(async () => {
      try {
        const res = await fetch('/graphql', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({
            query: `
              query PollState {
                activeNodes(userId: "${username}") {
                  deviceId
                  petname
                  clientType
                  ipAddress
                  version
                  currentTrack
                  lastSeenAt
                  isOnline
                }
                playbackHandoff(userId: "${username}") {
                  trackTitle
                  artistName
                  albumName
                  positionMs
                  isPlaying
                  deviceId
                }
              }
            `
          })
        });
        if (res.ok) {
          const { data } = await res.json();
          if (data?.activeNodes) {
            setNodes(data.activeNodes);
          }
          if (data?.playbackHandoff && data.playbackHandoff.trackTitle) {
            setLastHandoff(prev => {
              const cur = data.playbackHandoff;
              if (cur.trackTitle !== prev.title || cur.positionMs !== prev.positionMs || cur.isPlaying !== prev.isPlaying) {
                return {
                  ...prev,
                  title: cur.trackTitle,
                  artist: cur.artistName,
                  album: cur.albumName || "Unknown Album",
                  positionMs: cur.positionMs,
                  durationMs: 243000,
                  isPlaying: cur.isPlaying,
                  deviceId: cur.deviceId
                };
              }
              return prev;
            });
          }
        }
      } catch (_) {}
    }, 2500);

    // Connect WebSocket for real-time daemon events
    const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    const wsUrl = `${protocol}//${window.location.host}/ws/sync`;
    let ws;
    try {
      ws = new WebSocket(wsUrl);
      ws.onopen = () => {
        setSyncLogs(logs => [
          { time: new Date().toLocaleTimeString(), event: `[WS] Connected to live sync stream` },
          ...logs
        ]);
      };
      ws.onmessage = (evt) => {
        try {
          const parsed = JSON.parse(evt.data);
          if (parsed.msg_type === 'HANDOFF' && parsed.payload) {
            const p = parsed.payload;
            setLastHandoff({
              title: p.trackTitle || "Unknown Track",
              artist: p.artistName || "Unknown Artist",
              album: p.albumName || "Unknown Album",
              positionMs: p.positionMs || 0,
              durationMs: 243000,
              isPlaying: p.isPlaying ?? true,
              deviceId: p.deviceId || "Wander Desktop (TUI)"
            });
            setSyncLogs(logs => [
              { time: new Date().toLocaleTimeString(), event: `[HANDOFF] Transfer state from ${p.petname || p.deviceId}: "${p.trackTitle}" (${Math.floor((p.positionMs || 0) / 1000)}s)` },
              ...logs
            ]);
          } else if (parsed.msg_type === 'NODE_UPDATE' && parsed.payload) {
            setSyncLogs(logs => [
              { time: new Date().toLocaleTimeString(), event: `[PRESENCE] Node "${parsed.payload.petname}" (${parsed.payload.deviceId}) updated` },
              ...logs
            ]);
          } else {
            setSyncLogs(logs => [
              { time: new Date().toLocaleTimeString(), event: `[WS:${parsed.msg_type || 'EVENT'}] ${JSON.stringify(parsed.payload)}` },
              ...logs
            ]);
          }
        } catch (_) {
          setSyncLogs(logs => [
            { time: new Date().toLocaleTimeString(), event: `[WS] ${evt.data}` },
            ...logs
          ]);
        }
      };
    } catch (_) {}

    return () => {
      clearInterval(interval);
      if (ws) ws.close();
    };
  }, [username]);

  const serverHost = typeof window !== 'undefined' 
    ? (window.location.hostname === 'localhost' || window.location.hostname === '127.0.0.1'
        ? `http://${window.location.hostname}:8700`
        : window.location.origin)
    : 'http://127.0.0.1:8700';

  const qrPayload = `agro://connect?username=${encodeURIComponent(username)}&passphrase=${encodeURIComponent(passphrase)}&server=${encodeURIComponent(serverHost)}`;

  const handleCopy = () => {
    navigator.clipboard.writeText(passphrase);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  const handleRegenerate = async () => {
    try {
      const res = await fetch('/graphql', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          query: `
            mutation RotateAccount {
              createAccount(username: "${username}") {
                passphrase
              }
            }
          `
        })
      });
      if (res.ok) {
        const { data } = await res.json();
        if (data?.createAccount?.passphrase) {
          const newPass = data.createAccount.passphrase;
          setPassphrase(newPass);
          setSyncLogs(prev => [
            { time: new Date().toLocaleTimeString(), event: `[AUTH] Rotated natural passphrase: "${newPass}"` },
            ...prev
          ]);
          return;
        }
      }
    } catch (_) {}
  };

  const toggleRule = async (id) => {
    const targetRule = rules.find(r => r.id === id);
    if (!targetRule) return;
    const nextState = !targetRule.isEnabled;

    setRules(prev => prev.map(r => r.id === id ? { ...r, isEnabled: nextState } : r));

    setSyncLogs(logs => [
      { time: new Date().toLocaleTimeString(), event: `[RULE] ${targetRule.name}: ${nextState ? 'ENABLED' : 'DISABLED'}` },
      ...logs
    ]);

    try {
      await fetch('/graphql', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          query: `
            mutation TogglePluginState {
              togglePlugin(pluginId: "${id}", isEnabled: ${nextState})
            }
          `
        })
      });
    } catch (_) {}
  };

  const handleSaveSyncedSettings = async () => {
    try {
      const res = await fetch('/graphql', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          query: `
            mutation SaveSettings {
              updateSyncedSettings(input: {
                userId: "${username}",
                serverUrl: "${syncedSettings.serverUrl}",
                serverUsername: "${syncedSettings.serverUsername}",
                lrclibUrl: "${syncedSettings.lrclibUrl}",
                lyricsFetchOnline: ${syncedSettings.lyricsFetchOnline},
                streamFormat: "${syncedSettings.streamFormat}"
              }) {
                updatedAt
              }
            }
          `
        })
      });
      if (res.ok) {
        setSettingsSaved(true);
        setTimeout(() => setSettingsSaved(false), 2000);
        setSyncLogs(prev => [
          { time: new Date().toLocaleTimeString(), event: `[SETTINGS] Encrypted & broadcast updated settings for ${username}` },
          ...prev
        ]);
      }
    } catch (_) {}
  };

  const handleCreateUser = async () => {
    const clean = newUsernameInput.trim().toLowerCase();
    if (!clean) return;
    try {
      const res = await fetch('/graphql', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          query: `
            query InitNewUser {
              me(username: "${clean}") {
                username
                passphrase
              }
            }
          `
        })
      });
      if (res.ok) {
        setUsername(clean);
        setNewUsernameInput('');
        setShowUserMenu(false);
        if (!usersList.includes(clean)) {
          setUsersList(prev => [...prev, clean]);
        }
      }
    } catch (_) {}
  };

  const positionSec = Math.floor(lastHandoff.positionMs / 1000);
  const durationSec = Math.floor(lastHandoff.durationMs / 1000);

  return (
    <div className="app-wrapper">
      <div className="content-container">
        {/* Top Header with Multi-User Switcher */}
        <header className="top-header">
          <div className="brand-row" style={{ width: '100%', justifyContent: 'space-between', padding: '0 4px' }}>
            <div style={{ width: '110px' }} />
            <div className="brand-title">
              <Radio size={16} />
              <span>AGRO</span>
            </div>
            <div className="user-dropdown-container">
              <button className="user-badge-btn" onClick={() => setShowUserMenu(!showUserMenu)} title="Active User Tenant">
                <User size={13} />
                <span>{username}</span>
                <ChevronDown size={12} />
              </button>
              {showUserMenu && (
                <div className="user-dropdown-menu">
                  <div className="user-dropdown-title">User Tenant</div>
                  {usersList.map(u => (
                    <button 
                      key={u} 
                      className={`user-item ${u === username ? 'active' : ''}`}
                      onClick={() => { setUsername(u); setShowUserMenu(false); }}
                    >
                      <span>{u}</span>
                      {u === username && <Check size={12} />}
                    </button>
                  ))}
                  <div className="user-dropdown-divider" />
                  <div style={{ padding: '6px 8px', display: 'flex', gap: '6px' }}>
                    <input 
                      type="text" 
                      placeholder="Add user" 
                      value={newUsernameInput} 
                      onChange={e => setNewUsernameInput(e.target.value)}
                      onKeyDown={e => { if (e.key === 'Enter') handleCreateUser(); }}
                      style={{ width: '110px', padding: '4px 6px', fontSize: '11px', background: 'var(--bg-app)', border: '1px solid var(--border-strong)', borderRadius: '4px', color: 'var(--text-primary)' }}
                    />
                    <button className="btn btn-secondary" onClick={handleCreateUser} style={{ padding: '4px 8px', fontSize: '11px' }}>
                      <Plus size={12} />
                    </button>
                  </div>
                </div>
              )}
            </div>
          </div>

          {/* Centered Pill Bar Navigation */}
          <nav className="nav-pill-bar">
            <button 
              className={`nav-pill-btn ${activeTab === 'nodes' ? 'active' : ''}`}
              onClick={() => setActiveTab('nodes')}
            >
              <Server size={14} />
              <span>Nodes</span>
            </button>
            <button 
              className={`nav-pill-btn ${activeTab === 'rules' ? 'active' : ''}`}
              onClick={() => setActiveTab('rules')}
            >
              <Layers size={14} />
              <span>Plugins & Settings</span>
            </button>
            <button 
              className={`nav-pill-btn ${activeTab === 'pairing' ? 'active' : ''}`}
              onClick={() => setActiveTab('pairing')}
            >
              <KeyRound size={14} />
              <span>Pairing</span>
            </button>
            <button 
              className={`nav-pill-btn ${activeTab === 'logs' ? 'active' : ''}`}
              onClick={() => setActiveTab('logs')}
            >
              <ScrollText size={14} />
              <span>Logs</span>
            </button>
          </nav>
        </header>

        {/* Tab 1: Nodes & Playback State */}
        {activeTab === 'nodes' && (
          <div style={{ display: 'flex', flexDirection: 'column', gap: '16px' }}>
            <div className="card">
              <div className="card-header">
                <div className="card-title">Nodes ({nodes.length})</div>
              </div>

              {nodes.length === 0 ? (
                <div style={{ padding: '24px', textAlign: 'center', color: 'var(--text-muted)', fontSize: '13px', background: 'var(--bg-surface-elevated)', borderRadius: 'var(--radius-sm)' }}>
                  No client nodes registered yet. Launch <strong>wander</strong> or <strong>wanda</strong> to connect.
                </div>
              ) : (
                <div className="nodes-grid">
                  {nodes.map(node => (
                    <div key={node.deviceId} className="node-card">
                      <div className="node-header">
                        <div>
                          <div className="node-name">
                            {node.clientType.toLowerCase().includes('wanda') ? <Smartphone size={14} /> : <Terminal size={14} />}
                            <span>{node.petname}</span>
                          </div>
                          <div className="node-type">
                            {node.clientType.toLowerCase().includes('wanda') ? 'wanda' : 'wander'}
                          </div>
                        </div>
                        <span className="daemon-pill" style={{ 
                          fontSize: '10px', 
                          padding: '2px 6px',
                          color: node.isOnline ? 'var(--status-active)' : 'var(--text-muted)'
                        }}>
                          {node.isOnline ? 'ONLINE' : 'AWAY'}
                        </span>
                      </div>
                      <div className="node-footer">
                        <span>{node.currentTrack ? `Track: ${node.currentTrack}` : 'Status: Idle'}</span>
                      </div>
                    </div>
                  ))}
                </div>
              )}
            </div>

            {/* Playback State */}
            <div className="card">
              <div className="card-header">
                <div className="card-title">
                  <Disc size={15} /> Playback State
                </div>
              </div>

              <div style={{ background: 'var(--bg-surface-elevated)', padding: '14px 16px', borderRadius: 'var(--radius-sm)', border: '1px solid var(--border-subtle)', display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
                <div>
                  <div style={{ fontSize: '13px', fontWeight: 600 }}>{lastHandoff.title} • {lastHandoff.artist}</div>
                  <div style={{ fontSize: '11px', color: 'var(--text-muted)', fontFamily: 'JetBrains Mono, monospace', marginTop: '2px' }}>
                    {lastHandoff.isPlaying ? (
                      `Position: ${Math.floor(positionSec / 60)}:${String(positionSec % 60).padStart(2, '0')} • ${lastHandoff.album || 'Playing'}`
                    ) : (
                      'No playback actively broadcasting'
                    )}
                  </div>
                </div>
                <div style={{ textAlign: 'right' }}>
                  <span className="daemon-pill" style={{ fontSize: '10px' }}>
                    {lastHandoff.isPlaying ? `PLAYING ON ${lastHandoff.deviceId.toUpperCase()}` : 'PAUSED'}
                  </span>
                </div>
              </div>
            </div>
          </div>
        )}

        {/* Tab 2: Plugins & Synced Settings */}
        {activeTab === 'rules' && (
          <div style={{ display: 'flex', flexDirection: 'column', gap: '16px' }}>
            {/* Cross-Device Synced Settings */}
            <div className="card">
              <div className="card-header">
                <div className="card-title">
                  <Sliders size={15} /> Cross-Device Synced Settings
                </div>
                <button className="btn btn-secondary" onClick={handleSaveSyncedSettings}>
                  {settingsSaved ? <Check size={13} color="var(--status-active)" /> : <Save size={13} />}
                  <span>{settingsSaved ? 'Saved & Synced!' : 'Sync to Devices'}</span>
                </button>
              </div>

              <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: '12px', marginTop: '4px' }}>
                <div>
                  <label style={{ fontSize: '11px', color: 'var(--text-muted)', display: 'block', marginBottom: '4px' }}>
                    Subsonic / Navidrome URL
                  </label>
                  <input 
                    type="text" 
                    value={syncedSettings.serverUrl}
                    onChange={(e) => setSyncedSettings({ ...syncedSettings, serverUrl: e.target.value })}
                    style={{ width: '100%', padding: '8px 10px', background: 'var(--bg-surface-elevated)', border: '1px solid var(--border-subtle)', borderRadius: 'var(--radius-sm)', color: 'var(--text-main)', fontSize: '12px' }}
                  />
                </div>
                <div>
                  <label style={{ fontSize: '11px', color: 'var(--text-muted)', display: 'block', marginBottom: '4px' }}>
                    Server Username
                  </label>
                  <input 
                    type="text" 
                    value={syncedSettings.serverUsername}
                    onChange={(e) => setSyncedSettings({ ...syncedSettings, serverUsername: e.target.value })}
                    style={{ width: '100%', padding: '8px 10px', background: 'var(--bg-surface-elevated)', border: '1px solid var(--border-subtle)', borderRadius: 'var(--radius-sm)', color: 'var(--text-main)', fontSize: '12px' }}
                  />
                </div>
                <div>
                  <label style={{ fontSize: '11px', color: 'var(--text-muted)', display: 'block', marginBottom: '4px' }}>
                    LRCLIB Lyrics Server
                  </label>
                  <input 
                    type="text" 
                    value={syncedSettings.lrclibUrl}
                    onChange={(e) => setSyncedSettings({ ...syncedSettings, lrclibUrl: e.target.value })}
                    style={{ width: '100%', padding: '8px 10px', background: 'var(--bg-surface-elevated)', border: '1px solid var(--border-subtle)', borderRadius: 'var(--radius-sm)', color: 'var(--text-main)', fontSize: '12px' }}
                  />
                </div>
                <div>
                  <label style={{ fontSize: '11px', color: 'var(--text-muted)', display: 'block', marginBottom: '4px' }}>
                    Stream Audio Quality
                  </label>
                  <select 
                    value={syncedSettings.streamFormat}
                    onChange={(e) => setSyncedSettings({ ...syncedSettings, streamFormat: e.target.value })}
                    style={{ width: '100%', padding: '8px 10px', background: 'var(--bg-surface-elevated)', border: '1px solid var(--border-subtle)', borderRadius: 'var(--radius-sm)', color: 'var(--text-main)', fontSize: '12px' }}
                  >
                    <option value="FLAC">FLAC (Lossless Master)</option>
                    <option value="OPUS">Opus (High Efficiency)</option>
                    <option value="MP3">MP3 320k (Universal)</option>
                  </select>
                </div>
              </div>
            </div>

            {/* Sync Rules */}
            <div className="card">
              <div className="card-header">
                <div className="card-title">Plugins & Sync Rules</div>
              </div>

              <div className="rules-list">
                {rules.map(rule => (
                  <div key={rule.id} className={`rule-row ${!rule.isEnabled ? 'disabled' : ''}`}>
                    <div className="rule-info">
                      <div className="rule-title">
                        {rule.name}
                        <span className="rule-tag">{rule.target}</span>
                      </div>
                      <div className="rule-desc">{rule.description}</div>
                    </div>

                    <label className="switch">
                      <input 
                        type="checkbox" 
                        checked={rule.isEnabled} 
                        onChange={() => toggleRule(rule.id)} 
                      />
                      <span className="slider" />
                    </label>
                  </div>
                ))}
              </div>
            </div>
          </div>
        )}

        {/* Tab 3: Passphrase & Pairing */}
        {activeTab === 'pairing' && (
          <div className="card">
            <div className="card-header">
              <div className="card-title">Pairing</div>
              <button className="btn btn-secondary" onClick={handleRegenerate} title="Rotate 4-word passphrase">
                <RefreshCw size={13} />
                <span>Rotate Passphrase</span>
              </button>
            </div>

            <div className="pairing-container">
              <div className="qr-box">
                <QRCodeSVG value={qrPayload} size={150} level="M" />
              </div>

              <div style={{ width: '100%', display: 'flex', flexDirection: 'column', alignItems: 'center', gap: '6px' }}>
                <div style={{ fontSize: '11px', color: 'var(--text-muted)', textTransform: 'uppercase', letterSpacing: '0.05em' }}>
                  Natural Passphrase
                </div>
                <div className="passphrase-display">
                  <span>{passphrase}</span>
                  <button className="btn btn-secondary" onClick={handleCopy} style={{ padding: '4px 8px' }}>
                    {copied ? <Check size={13} color="var(--status-active)" /> : <Copy size={13} />}
                  </button>
                </div>
              </div>

              <div className="code-snippet">
                <div style={{ color: 'var(--text-muted)', marginBottom: '4px' }}># Wander TUI (~/.config/wander/config.toml)</div>
                <div>[agro]</div>
                <div>enabled = true</div>
                <div>server = "http://127.0.0.1:8700"</div>
                <div>username = "{username}"</div>
                <div>passphrase = "{passphrase}"</div>
              </div>
            </div>
          </div>
        )}

        {/* Tab 4: Daemon Logs */}
        {activeTab === 'logs' && (
          <div className="card">
            <div className="card-header">
              <div className="card-title">Logs</div>
            </div>

            <div className="terminal-card">
              {syncLogs.map((log, idx) => (
                <div key={idx} className="terminal-line">
                  <span className="terminal-ts">[{log.time}]</span>
                  <span>{log.event}</span>
                </div>
              ))}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
