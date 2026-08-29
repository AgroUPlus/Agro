import React, { useState, useEffect } from 'react';
import { QRCodeSVG } from 'qrcode.react';
import { 
  Smartphone, Terminal, Server, 
  Layers, KeyRound, ScrollText, Copy, 
  Check, RefreshCw, Disc, Sliders, Save,
  User, Users, ChevronDown, Plus, Library, HardDrive, Trash2,
  Music, Database, Activity, Link2 as LinkIcon, BarChart3, UserPlus, Pencil, Inbox } from 'lucide-react';
import LinksTab from './tabs/LinksTab.jsx';
import FeedTab from './tabs/FeedTab.jsx';
import InboxTab from './tabs/InboxTab.jsx';
import LibraryBrowser from './tabs/LibraryBrowser.jsx';
import StatsTab from './tabs/StatsTab.jsx';
import PeopleTab from './tabs/PeopleTab.jsx';
import AuthScreen from './AuthScreen.jsx';
import DevicesTab from './tabs/DevicesTab.jsx';
import Avatar from './Avatar.jsx';

/** Bytes as something readable. The library totals are the only place this is needed. */
function formatBytes(bytes) {
  if (!bytes) return '0 MB';
  const gb = 1024 ** 3;
  const mb = 1024 ** 2;
  if (bytes >= gb) return `${(bytes / gb).toFixed(1)} GB`;
  return `${Math.round(bytes / mb)} MB`;
}

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
    id: 'listen-along',
    name: 'Listen Along',
    description: 'Pushes a host\'s track and position to the friends following them, so a session can be joined rather than merely watched. Gated on the host\'s "show now playing" switch.',
    target: 'Multi-device',
    isEnabled: true
  }
];


/**
 * The API requires a bearer token. It is kept in localStorage rather than in React state so a
 * reload does not log you out, and it is sent on every request from one place — a fetch that
 * forgets the header now fails loudly with a 401 rather than silently returning someone's data.
 */
const TOKEN_KEY = 'agro.token';

export function getToken() {
  return localStorage.getItem(TOKEN_KEY) || '';
}

export function setToken(value) {
  if (value) localStorage.setItem(TOKEN_KEY, value.trim());
  else localStorage.removeItem(TOKEN_KEY);
}

/**
 * POSTs a GraphQL document with the stored token. Throws on 401 so callers can prompt.
 *
 * `variables` is optional only because the calls that predate it interpolate the username straight
 * into the document. New callers should pass variables instead: a link id or a username spliced
 * into a query string is an injection waiting to happen, and it breaks outright on any value
 * containing a quote.
 */
/**
 * Exchanges a passphrase for a device token and stores it.
 *
 * The passphrase used to *be* the token: whatever you typed was put straight into the
 * `Authorization` header. It no longer is — it buys a revocable per-device credential from
 * `/api/v1/login`, which is one of only two routes reachable without a token.
 */
export async function login(username, passphrase) {
  const res = await fetch('/api/v1/login', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ username: username.trim(), passphrase, label: 'dashboard' })
  });
  const body = await res.json().catch(() => ({}));
  if (!res.ok || !body.token) {
    throw new Error(body.error || 'Those credentials were not accepted');
  }
  setToken(body.token);
  return body;
}

export async function gql(query, variables) {
  const res = await fetch('/graphql', {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      ...(getToken() ? { Authorization: `Bearer ${getToken()}` } : {})
    },
    body: JSON.stringify(variables ? { query, variables } : { query })
  });
  if (res.status === 401) {
    const error = new Error('Unauthorized');
    error.unauthorized = true;
    throw error;
  }
  return res;
}

/**
 * The navigation, as data.
 *
 * The sidebar renders this and the page heading reads from it, so a tab cannot be labelled one
 * thing in the nav and another at the top of its own page. `adminOnly` hides what a member's token
 * would be refused anyway — showing a control that always errors is worse than not showing it.
 */
const NAV_ITEMS = [
  { id: 'feed', label: 'Feed', icon: Activity },
  { id: 'inbox', label: 'Inbox', icon: Inbox },
  { id: 'nodes', label: 'Devices', icon: Server },
  { id: 'library', label: 'Library', icon: Library },
  { id: 'stats', label: 'Stats', icon: BarChart3 },
  { id: 'links', label: 'Links', icon: LinkIcon },
  { id: 'pairing', label: 'Sign-ins', icon: KeyRound },
  { id: 'people', label: 'People', icon: UserPlus, adminOnly: true },
  { id: 'rules', label: 'Plugins & Settings', icon: Layers, adminOnly: true },
  { id: 'logs', label: 'Logs', icon: ScrollText, adminOnly: true }
];

export default function App() {
  // Null until the first request tells us whether the stored token works. Rendering the
  // dashboard before then would flash real-looking empty data at someone who is not signed in.
  const [locked, setLocked] = useState(!getToken());
  const [activeTab, setActiveTab] = useState('nodes');
  // Drops that have arrived over the socket since this page was opened.
  //
  // A badge, not a source of truth: the server's `unreadDropCount` is the real number, and opening
  // the Inbox clears this so the two cannot drift apart on screen.
  const [unreadDrops, setUnreadDrops] = useState(0);
  // Empty, not a guess. This used to start at 'alpha', which was then interpolated into the very
  // first query — so signing in as anyone else asked `me(username: "alpha")`, got refused, and
  // left the page displaying a name that was not yours over a session that was.
  const [username, setUsername] = useState('');
  const [role, setRole] = useState('');
  const [showUserMenu, setShowUserMenu] = useState(false);
  // Administrator-only surfaces. A member asking for `users` or `plugins` fails the whole
  // document, so this gates the query as well as the navigation.
  const isAdmin = role === 'admin';
  const [deviceNameInput, setDeviceNameInput] = useState('');
  // The pairing payload, minted on demand. It used to be built here from the account passphrase,
  // which meant the QR on screen *was* the account: photographing it handed over everything,
  // permanently. Each scan now gets its own revocable device token.
  const [pairing, setPairing] = useState(null);
  const [copied, setCopied] = useState(false);
  const [rules, setRules] = useState(FALLBACK_RULES);
  const [nodes, setNodes] = useState([]);
  // Library index totals, refreshed alongside everything else on the poll.
  const [libraryStats, setLibraryStats] = useState(null);
  const [syncedSettings, setSyncedSettings] = useState({
    serverUrl: 'http://localhost:4533',
    serverUsername: 'alpha',
    lrclibUrl: 'https://lrclib.net',
    lyricsFetchOnline: true,
    jamendoClientId: '',
    streamFormat: 'FLAC',
    // Share-link forwarding. Blank domain and enabled:false is "off", which is also what every
    // player does with no Agro at all — the feature is an addition, never a dependency.
    shareDomain: '',
    shareHosts: '',
    shareEnabled: false
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

  // Who is signed in. Asked first and on its own, because every other query has to name an
  // account and there is nothing to name until this answers.
  useEffect(() => {
    if (locked) return;
    let cancelled = false;
    (async () => {
      try {
        const res = await gql(`{ me { username role state } }`);
        const { data } = await res.json();
        if (!cancelled && data?.me?.username) {
          setUsername(data.me.username);
          setRole(data.me.role || '');
        }
      } catch (e) {
        if (e.unauthorized) setLocked(true);
      }
    })();
    return () => { cancelled = true; };
  }, [locked]);

  // Query live user passphrase, plugins, nodes, and handoff from Agro GraphQL backend
  useEffect(() => {
    // Nothing can be keyed on an account before we know which one.
    if (!username) return;
    async function loadBackendData() {
      try {
        const res = await gql(`
              query LoadInitialState {
                ${isAdmin ? `plugins { id name description target isEnabled }` : ''}
                me { username role }
                activeNodes(userId: "${username}") {
                  deviceId
                  petname
                  clientType
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
                  shareDomain
                  shareHosts
                  shareEnabled
                }
                libraryStats(userId: "${username}") {
                  trackCount
                  archivedCount
                  totalBytes
                  spoolBytes
                }
              }
            `);

        if (res.ok) {
          const { data } = await res.json();
          if (data?.me) {
            setUsername(data.me.username);
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
          if (data?.libraryStats) {
            setLibraryStats(data.libraryStats);
          }
          if (data?.syncedSettings) {
            setSyncedSettings(s => ({
              ...s,
              serverUrl: data.syncedSettings.serverUrl || s.serverUrl,
              serverUsername: data.syncedSettings.serverUsername || s.serverUsername,
              lrclibUrl: data.syncedSettings.lrclibUrl || s.lrclibUrl,
              lyricsFetchOnline: data.syncedSettings.lyricsFetchOnline ?? s.lyricsFetchOnline,
              jamendoClientId: data.syncedSettings.jamendoClientId || '',
              streamFormat: data.syncedSettings.streamFormat || 'FLAC',
              shareDomain: data.syncedSettings.shareDomain || '',
              shareHosts: data.syncedSettings.shareHosts || '',
              shareEnabled: data.syncedSettings.shareEnabled ?? false
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
      } catch (e) { if (e.unauthorized) setLocked(true); }
    }

    loadBackendData();

    // Polling every 2.5 seconds to refresh state & nodes
    const interval = setInterval(async () => {
      try {
        const res = await gql(`
              query PollState {
                activeNodes(userId: "${username}") {
                  deviceId
                  petname
                  clientType
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
                libraryStats(userId: "${username}") {
                  trackCount
                  archivedCount
                  totalBytes
                  spoolBytes
                }
              }
            `);
        if (res.ok) {
          const { data } = await res.json();
          if (data?.activeNodes) {
            setNodes(data.activeNodes);
          }
          if (data?.libraryStats) {
            setLibraryStats(data.libraryStats);
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
      } catch (e) { if (e.unauthorized) setLocked(true); }
    }, 2500);

    // Connect WebSocket for real-time daemon events
    const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    // The token has to travel: a browser cannot set headers on a WebSocket handshake, so the
    // server also accepts it as a query parameter. Without it the middleware rejects the upgrade
    // and the Logs tab is silently dead in any deployment that has an account.
    const wsUrl = `${protocol}//${window.location.host}/ws/sync?token=${encodeURIComponent(getToken() || '')}`;
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
          } else if (parsed.msg_type === 'TRACK_DROP' && parsed.payload) {
            const p = parsed.payload;
            // Counted here rather than re-queried: the frame carries everything the badge needs,
            // and the Inbox tab reads the authoritative number when it opens. A badge that had to
            // round-trip would lag behind the notification that caused it.
            setUnreadDrops(count => count + 1);
            setSyncLogs(logs => [
              { time: new Date().toLocaleTimeString(), event: `[DROP] ${p.from} sent "${p.trackTitle}" by ${p.artistName}` },
              ...logs
            ]);
          } else if (parsed.msg_type === 'FRIEND_PRESENCE' && parsed.payload) {
            const p = parsed.payload;
            setSyncLogs(logs => [
              { time: new Date().toLocaleTimeString(), event: `[FRIEND] ${p.username} ${p.isPlaying ? 'is playing' : 'paused'} "${p.trackTitle}"` },
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
    } catch (e) { if (e.unauthorized) setLocked(true); }

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

  const qrPayload = pairing?.qrData || '';

  const handleCopy = () => {
    if (!qrPayload) return;
    navigator.clipboard.writeText(qrPayload);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  /** Mints a fresh pairing token. Nothing is on screen until this is asked for. */
  const handlePair = async () => {
    // Named by the person pairing, because only they know what the device is. Unnamed tokens all
    // arrived called "paired device", which is no help at all in a list of eight of them.
    const name = deviceNameInput.trim();
    if (!name) return;
    try {
      const res = await gql(
        `mutation PairDevice($u: String!, $label: String) { pairDevice(userId: $u, label: $label) { qrData token label } }`,
        { u: username, label: name }
      );
      const { data } = await res.json();
      if (data?.pairDevice) {
        setPairing(data.pairDevice);
        setDeviceNameInput('');
        setSyncLogs((prev) => [
          { time: new Date().toLocaleTimeString(), event: `[AUTH] Issued a pairing token named “${name}”` },
          ...prev
        ]);
      }
    } catch (e) { if (e.unauthorized) setLocked(true); }
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
      await gql(`
            mutation TogglePluginState {
              togglePlugin(pluginId: "${id}", isEnabled: ${nextState})
            }
          `);
    } catch (e) { if (e.unauthorized) setLocked(true); }
  };

  const handleSaveSyncedSettings = async () => {
    try {
      const res = await gql(`
            mutation SaveSettings {
              updateSyncedSettings(input: {
                userId: "${username}",
                serverUrl: "${syncedSettings.serverUrl}",
                serverUsername: "${syncedSettings.serverUsername}",
                lrclibUrl: "${syncedSettings.lrclibUrl}",
                lyricsFetchOnline: ${syncedSettings.lyricsFetchOnline},
                streamFormat: "${syncedSettings.streamFormat}",
                shareDomain: "${syncedSettings.shareDomain}",
                shareHosts: "${syncedSettings.shareHosts}",
                shareEnabled: ${syncedSettings.shareEnabled}
              }) {
                updatedAt
              }
            }
          `);
      if (res.ok) {
        setSettingsSaved(true);
        setTimeout(() => setSettingsSaved(false), 2000);
        setSyncLogs(prev => [
          { time: new Date().toLocaleTimeString(), event: `[SETTINGS] Encrypted & broadcast updated settings for ${username}` },
          ...prev
        ]);
      }
    } catch (e) { if (e.unauthorized) setLocked(true); }
  };

  /**
   * Renames a device.
   *
   * Until now the only way to change a device's name was to make the client send a different one —
   * which for a name the server invented meant there was no way at all, and left people staring at
   * a "Caffeinated Panda" they never chose.
   */
  const handleRenameNode = async (node) => {
    const typed = window.prompt(`What should this device be called?`, node.petname);
    if (typed === null) return;
    const petname = typed.trim();
    if (!petname) return;
    try {
      const res = await gql(
        `mutation RenameNode($u: String!, $d: String!, $p: String!) {
           renameNode(userId: $u, deviceId: $d, petname: $p)
         }`,
        { u: username, d: node.deviceId, p: petname }
      );
      const body = await res.json();
      if (body?.errors?.length) throw new Error(body.errors[0].message);
      setNodes((prev) => prev.map((n) =>
        n.deviceId === node.deviceId ? { ...n, petname } : n
      ));
    } catch (e) {
      if (e.unauthorized) setLocked(true);
    }
  };

  const handleDeleteNode = async (deviceId) => {
    try {
      await gql(`
        mutation UnregisterNode {
          unregisterNode(userId: "${username}", deviceId: "${deviceId}")
        }
      `);
      setNodes(prev => prev.filter(n => n.deviceId !== deviceId));
      setSyncLogs(prev => [
        { time: new Date().toLocaleTimeString(), event: `[NODE] Removed device ${deviceId}` },
        ...prev
      ]);
    } catch (e) { if (e.unauthorized) setLocked(true); }
  };

  const positionSec = Math.floor(lastHandoff.positionMs / 1000);
  const durationSec = Math.floor(lastHandoff.durationMs / 1000);

  if (locked) {
    return (
      <AuthScreen
        onSignedIn={() => {
          setLocked(false);
          window.location.reload();
        }}
      />
    );
  }

  return (
    <div className="app-shell">
      {/* Persistent navigation. A sidebar rather than a pill row: the tab list has grown to eight
          and a centred row of pills had started wrapping and shrinking its own labels. */}
      <aside className="sidebar">
        <div className="sidebar-brand">Agro</div>

        <nav className="sidebar-nav">
          {NAV_ITEMS.filter((item) => !item.adminOnly || isAdmin).map((item) => (
            <button
              key={item.id}
              className={`nav-item ${activeTab === item.id ? 'active' : ''}`}
              onClick={() => {
                setActiveTab(item.id);
                // Opening the Inbox is what makes its badge stale, so it is also what clears it.
                if (item.id === 'inbox') setUnreadDrops(0);
              }}
            >
              <item.icon size={18} />
              <span>{item.label}</span>
              {item.id === 'inbox' && unreadDrops > 0 && (
                <span className="nav-badge">{unreadDrops}</span>
              )}
            </button>
          ))}
        </nav>

        <div className="sidebar-footer">
          <div className="user-dropdown-container">
            <button className="user-badge-btn" onClick={() => setShowUserMenu(!showUserMenu)}>
              <Avatar username={username} size={22} />
              <span className="user-badge-name">{username || '…'}</span>
              {isAdmin && <span className="role-chip">admin</span>}
              <ChevronDown size={14} />
            </button>
            {showUserMenu && (
              <div className="user-dropdown-menu">
                {/* No account switcher. It set a local variable and re-keyed every query to
                    someone else's name, which the server then refused — you cannot act as another
                    account by choosing it from a menu, and a control that implies you can is worse
                    than no control. You are whoever the token says you are.

                    No "create account" either: accounts come from /api/v1/signup and the approval
                    queue, so that the same checks apply to everyone. */}
                <div className="user-dropdown-row">
                  <button
                    className="btn btn-secondary btn-block"
                    onClick={() => { setToken(''); setLocked(true); }}
                  >
                    Sign out
                  </button>
                </div>
              </div>
            )}
          </div>
        </div>
      </aside>

      <main className="main-area">
        <header className="page-header">
          <h1>{NAV_ITEMS.find((item) => item.id === activeTab)?.label ?? 'Agro'}</h1>
        </header>

        <div className="page-content">
        {activeTab === 'feed' && (
          <FeedTab onUnauthorized={() => setLocked(true)} />
        )}

        {activeTab === 'inbox' && (
          <InboxTab onUnauthorized={() => setLocked(true)} />
        )}

        {activeTab === 'stats' && (
          <StatsTab username={username} nodes={nodes} onUnauthorized={() => setLocked(true)} />
        )}

        {activeTab === 'links' && (
          <LinksTab username={username} onUnauthorized={() => setLocked(true)} />
        )}

        {activeTab === 'people' && (
          <PeopleTab me={username} onUnauthorized={() => setLocked(true)} />
        )}

        {/* Tab 1: Nodes & Playback State */}
        {activeTab === 'nodes' && (
          <div style={{ display: 'flex', flexDirection: 'column', gap: '16px' }}>
            <div className="card">
              <div className="card-header">
                <div>
                  <div className="card-title">Devices ({nodes.length})</div>
                  <div className="card-subtitle">Apps signed in and reporting what they play</div>
                </div>
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
                        <div style={{ display: 'flex', alignItems: 'center', gap: '8px' }}>
                          <span className="daemon-pill" style={{ 
                            fontSize: '10px', 
                            padding: '2px 6px',
                            color: node.isOnline ? 'var(--status-active)' : 'var(--text-muted)'
                          }}>
                            {node.isOnline ? 'ONLINE' : 'AWAY'}
                          </span>
                          <button
                            onClick={() => handleRenameNode(node)}
                            title="Rename this device"
                            style={{
                              background: 'transparent',
                              border: 'none',
                              color: 'var(--text-muted)',
                              cursor: 'pointer',
                              padding: '2px',
                              display: 'flex',
                              alignItems: 'center'
                            }}
                          >
                            <Pencil size={13} />
                          </button>
                          <button
                            onClick={() => handleDeleteNode(node.deviceId)}
                            title="Remove device"
                            style={{
                              background: 'transparent',
                              border: 'none',
                              color: 'var(--text-muted)',
                              cursor: 'pointer',
                              padding: '2px',
                              display: 'flex',
                              alignItems: 'center'
                            }}
                          >
                            <Trash2 size={13} />
                          </button>
                        </div>
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

            {/* Share link forwarding */}
            <div className="card">
              <div className="card-header">
                <div className="card-title">Share Links</div>
                <button
                  className={`btn ${syncedSettings.shareEnabled ? 'btn-primary' : ''}`}
                  onClick={() => setSyncedSettings({ ...syncedSettings, shareEnabled: !syncedSettings.shareEnabled })}
                >
                  <span>{syncedSettings.shareEnabled ? 'On' : 'Off'}</span>
                </button>
              </div>

              <p style={{ fontSize: '12px', color: 'var(--text-muted)', margin: '0 0 12px' }}>
                When enabled, share links generated by players (like Wanda and Wander) will use your
                custom domain routed through <code>/listen</code> instead of pointing directly to the
                backend music server.
              </p>

              <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: '12px' }}>
                <div>
                  <label style={{ fontSize: '11px', color: 'var(--text-muted)', display: 'block', marginBottom: '4px' }}>
                    Share Domain
                  </label>
                  <input
                    type="text"
                    placeholder="share.example.com"
                    value={syncedSettings.shareDomain}
                    onChange={(e) => setSyncedSettings({ ...syncedSettings, shareDomain: e.target.value })}
                    style={{ width: '100%', padding: '8px 10px', background: 'var(--bg-surface-elevated)', border: '1px solid var(--border-subtle)', borderRadius: 'var(--radius-sm)', color: 'var(--text-main)', fontSize: '12px' }}
                  />
                  <div style={{ fontSize: '11px', color: 'var(--text-muted)', marginTop: '4px' }}>
                    Point its DNS at this server. Links read {syncedSettings.shareDomain || 'your-domain'}/listen?v=&hellip;
                  </div>
                </div>
                <div>
                  <label style={{ fontSize: '11px', color: 'var(--text-muted)', display: 'block', marginBottom: '4px' }}>
                    Forward To (comma separated)
                  </label>
                  <input
                    type="text"
                    placeholder="music.example.com"
                    value={syncedSettings.shareHosts}
                    onChange={(e) => setSyncedSettings({ ...syncedSettings, shareHosts: e.target.value })}
                    style={{ width: '100%', padding: '8px 10px', background: 'var(--bg-surface-elevated)', border: '1px solid var(--border-subtle)', borderRadius: 'var(--radius-sm)', color: 'var(--text-main)', fontSize: '12px' }}
                  />
                  <div style={{ fontSize: '11px', color: 'var(--text-muted)', marginTop: '4px' }}>
                    Your music server. YouTube&rsquo;s hosts are always allowed; anything else is
                    refused, so the domain cannot be used to forward strangers elsewhere.
                  </div>
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

        {/* Tab 3: Pairing */}
        {activeTab === 'pairing' && (
        <div style={{ display: 'flex', flexDirection: 'column', gap: '16px' }}>
          <div className="card">
            <div className="card-header">
              <div className="card-title">Pairing</div>
              <div className="pair-issue">
                <input
                  type="text"
                  placeholder="Device name, e.g. Living room laptop"
                  value={deviceNameInput}
                  onChange={(e) => setDeviceNameInput(e.target.value)}
                  onKeyDown={(e) => { if (e.key === 'Enter') handlePair(); }}
                />
                <button
                  className="btn btn-secondary"
                  onClick={handlePair}
                  disabled={!deviceNameInput.trim()}
                  title="Issue a device token under this name"
                >
                  <RefreshCw size={15} />
                  <span>Issue token</span>
                </button>
              </div>
            </div>

            {!pairing ? (
              <p className="empty-hint" style={{ padding: '24px' }}>
                Generate a token above to pair a new device. Each device gets its own token that can
                be revoked individually at any time.
              </p>
            ) : (
              <div className="pairing-container">
                <div className="qr-box">
                  <QRCodeSVG value={qrPayload} size={150} level="M" />
                </div>

                <div style={{ width: '100%', display: 'flex', flexDirection: 'column', alignItems: 'center', gap: '6px' }}>
                  <div style={{ fontSize: '11px', color: 'var(--text-muted)', textTransform: 'uppercase', letterSpacing: '0.05em' }}>
                    Device token — shown once
                  </div>
                  <div className="passphrase-display">
                    <span style={{ wordBreak: 'break-all' }}>{pairing.token}</span>
                    <button className="btn btn-secondary" onClick={handleCopy} style={{ padding: '4px 8px' }}>
                      {copied ? <Check size={13} color="var(--status-active)" /> : <Copy size={13} />}
                    </button>
                  </div>
                  <div style={{ fontSize: '11px', color: 'var(--text-muted)' }}>
                    Revoke it any time under app passwords, as “{pairing.label}”.
                  </div>
                </div>

                <div className="code-snippet">
                  <div style={{ color: 'var(--text-muted)', marginBottom: '4px' }}># Wander TUI (~/.config/wander/config.toml)</div>
                  <div>[agro]</div>
                  <div>enabled = true</div>
                  <div>server = "{serverHost}"</div>
                  <div>username = "{username}"</div>
                  <div>token = "{pairing.token}"</div>
                </div>
              </div>
            )}
          </div>
          <DevicesTab username={username} onUnauthorized={() => setLocked(true)} />
        </div>
        )}

        {/* Tab 4: Library */}
        {activeTab === 'library' && (
          <div style={{ display: 'flex', flexDirection: 'column', gap: '16px' }}>
            <div className="card">
              <div className="card-header">
                <div>
                  <div className="card-title">Music Library & Fleet Ledger</div>
                  <div className="card-subtitle">
                    Cross-device SHA-256 track index and local server audio archive
                  </div>
                </div>
              </div>

              {libraryStats ? (
                <>
                  <div className="nodes-grid" style={{ gridTemplateColumns: 'repeat(auto-fit, minmax(180px, 1fr))' }}>
                    <div className="node-card" style={{ padding: '16px' }}>
                      <div className="node-header" style={{ marginBottom: '8px' }}>
                        <span style={{ fontSize: '12px', color: 'var(--text-muted)', fontWeight: '500' }}>KNOWN IN FLEET</span>
                        <Music size={15} style={{ color: 'var(--text-secondary)' }} />
                      </div>
                      <div style={{ fontSize: '24px', fontWeight: '700', color: 'var(--text-primary)', fontFamily: 'JetBrains Mono, monospace' }}>
                        {libraryStats.trackCount}
                      </div>
                      <div style={{ fontSize: '11px', color: 'var(--text-muted)', marginTop: '4px' }}>
                        Indexed across nodes
                      </div>
                    </div>

                    <div className="node-card" style={{ padding: '16px' }}>
                      <div className="node-header" style={{ marginBottom: '8px' }}>
                        <span style={{ fontSize: '12px', color: 'var(--text-muted)', fontWeight: '500' }}>SERVER VAULT</span>
                        <HardDrive size={15} style={{ color: 'var(--status-active)' }} />
                      </div>
                      <div style={{ fontSize: '24px', fontWeight: '700', color: 'var(--status-active)', fontFamily: 'JetBrains Mono, monospace' }}>
                        {libraryStats.archivedCount}
                      </div>
                      <div style={{ fontSize: '11px', color: 'var(--text-muted)', marginTop: '4px' }}>
                        Stored on this server
                      </div>
                    </div>

                    <div className="node-card" style={{ padding: '16px' }}>
                      <div className="node-header" style={{ marginBottom: '8px' }}>
                        <span style={{ fontSize: '12px', color: 'var(--text-muted)', fontWeight: '500' }}>AUDIO STORAGE</span>
                        <Database size={15} style={{ color: 'var(--text-secondary)' }} />
                      </div>
                      <div style={{ fontSize: '24px', fontWeight: '700', color: 'var(--text-primary)', fontFamily: 'JetBrains Mono, monospace' }}>
                        {formatBytes(libraryStats.totalBytes)}
                      </div>
                      <div style={{ fontSize: '11px', color: 'var(--text-muted)', marginTop: '4px' }}>
                        Total library audio size
                      </div>
                    </div>

                    <div className="node-card" style={{ padding: '16px' }}>
                      <div className="node-header" style={{ marginBottom: '8px' }}>
                        <span style={{ fontSize: '12px', color: 'var(--text-muted)', fontWeight: '500' }}>PEER RELAY SPOOL</span>
                        <Activity size={15} style={{ color: 'var(--text-secondary)' }} />
                      </div>
                      <div style={{ fontSize: '24px', fontWeight: '700', color: 'var(--text-primary)', fontFamily: 'JetBrains Mono, monospace' }}>
                        {formatBytes(libraryStats.spoolBytes)}
                      </div>
                      <div style={{ fontSize: '11px', color: 'var(--text-muted)', marginTop: '4px' }}>
                        Staged for peer downloads
                      </div>
                    </div>
                  </div>

                </>
              ) : (
                <div style={{ padding: '32px 20px', textAlign: 'center', color: 'var(--text-muted)', fontSize: '13px', background: 'var(--bg-surface-elevated)', borderRadius: 'var(--radius-sm)' }}>
                  No tracks reported yet. Turn on <strong>Library Sync</strong> in Wanda or Wander to populate this ledger.
                </div>
              )}
            </div>

            <LibraryBrowser
              username={username}
              devices={nodes}
              onUnauthorized={() => setLocked(true)}
            />
          </div>
        )}

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
      </main>

      {/* Now playing, as a bar across the bottom of the shell rather than a card inside one tab:
          it is the one piece of state that stays true whichever screen you are looking at. */}
      <footer className="now-bar">
        <div className="now-bar-track">
          <div className="now-bar-title">{lastHandoff.title}</div>
          <div className="now-bar-meta">
            {lastHandoff.artist}
            {lastHandoff.album ? ` — ${lastHandoff.album}` : ''}
          </div>
        </div>
        <div className="now-bar-status">
          <span className={`status-dot ${lastHandoff.isPlaying ? 'is-on' : ''}`} />
          <span>{lastHandoff.isPlaying ? 'Playing' : 'Idle'}</span>
          {lastHandoff.isPlaying && (
            <span className="now-bar-time">
              {Math.floor(positionSec / 60)}:{String(positionSec % 60).padStart(2, '0')}
            </span>
          )}
          <span className="now-bar-device">
            {nodes.find((n) => n.deviceId === lastHandoff.deviceId)?.petname || lastHandoff.deviceId}
          </span>
        </div>
      </footer>
    </div>
  );
}
