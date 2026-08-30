import React, { useState, useEffect, useCallback } from 'react';
import {
  getToken,
  setToken,
  gql,
  consumeSsoFragment,
  FALLBACK_RULES
} from './api.js';
import Sidebar, { NAV_ITEMS } from './components/Sidebar.jsx';
import NowBar from './components/NowBar.jsx';
import AuthScreen from './AuthScreen.jsx';

import SocialTab from './tabs/SocialTab.jsx';
import DevicesTab from './tabs/DevicesTab.jsx';
import StatsTab from './tabs/StatsTab.jsx';
import LibraryBrowser from './tabs/LibraryBrowser.jsx';
import LinksTab from './tabs/LinksTab.jsx';
import AccountSettingsTab from './tabs/AccountSettingsTab.jsx';
import PeopleTab from './tabs/PeopleTab.jsx';
import AdminPluginsTab from './tabs/AdminPluginsTab.jsx';
import LogsTab from './tabs/LogsTab.jsx';

function getTabFromHash() {
  const hash = window.location.hash.replace(/^#\/?/, '').trim();
  const valid = NAV_ITEMS.map((item) => item.id);
  return valid.includes(hash) ? hash : 'social';
}

/**
 * Reads an SSO result out of the URL fragment before anything else renders.
 *
 * Run during the first render rather than in an effect, so the app never paints the signed-out
 * screen for a frame on the way back from the identity provider. `consumeSsoFragment` clears the
 * fragment as it reads it, so a token cannot linger in the address bar or the history.
 */
const ssoResult = consumeSsoFragment();

export default function App() {
  const [ssoError, setSsoError] = useState(ssoResult?.error || '');
  const [locked, setLocked] = useState(!getToken());
  const [activeTab, setActiveTab] = useState(getTabFromHash());
  const [unreadDrops, setUnreadDrops] = useState(0);
  const [username, setUsername] = useState('');
  const [role, setRole] = useState('');
  const isAdmin = role === 'admin';

  const [nodes, setNodes] = useState([]);
  const [rules, setRules] = useState(FALLBACK_RULES);
  const [syncedSettings, setSyncedSettings] = useState({
    serverUrl: 'http://localhost:4533',
    serverUsername: 'alpha',
    lrclibUrl: 'https://lrclib.net',
    lyricsFetchOnline: true,
    streamFormat: 'FLAC',
    shareDomain: '',
    shareHosts: '',
    shareEnabled: true
  });
  const [lastHandoff, setLastHandoff] = useState({
    title: 'Wander Daemon Ready',
    artist: 'Kolb Audio Subsystem',
    album: '',
    artworkUrl: '',
    positionMs: 0,
    durationMs: 0,
    isPlaying: false,
    deviceId: 'fleet'
  });
  const [syncLogs, setSyncLogs] = useState([]);

  // Hash-based URL routing
  useEffect(() => {
    const handleHashChange = () => {
      setActiveTab(getTabFromHash());
    };
    window.addEventListener('hashchange', handleHashChange);
    return () => window.removeEventListener('hashchange', handleHashChange);
  }, []);

  const handleTabSelect = (tabId) => {
    setActiveTab(tabId);
    window.location.hash = `#/${tabId}`;
    if (tabId === 'social') setUnreadDrops(0);
  };

  const handleSignOut = () => {
    setToken('');
    setLocked(true);
  };

  const handleRenameNode = async (node) => {
    const typed = window.prompt('What should this device be called?', node.petname);
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
      setNodes((prev) => prev.map((n) => (n.deviceId === node.deviceId ? { ...n, petname } : n)));
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
      setNodes((prev) => prev.filter((n) => n.deviceId !== deviceId));
      setSyncLogs((prev) => [
        { time: new Date().toLocaleTimeString(), event: `[NODE] Removed device ${deviceId}` },
        ...prev
      ]);
    } catch (e) {
      if (e.unauthorized) setLocked(true);
    }
  };

  const handleToggleRule = async (id) => {
    const target = rules.find((r) => r.id === id);
    if (!target) return;
    const nextState = !target.isEnabled;
    setRules((prev) => prev.map((r) => (r.id === id ? { ...r, isEnabled: nextState } : r)));
    try {
      await gql(`mutation TogglePluginState { togglePlugin(pluginId: "${id}", isEnabled: ${nextState}) }`);
    } catch (e) {
      if (e.unauthorized) setLocked(true);
    }
  };

  const poll = useCallback(async () => {
    if (!getToken()) return;
    try {
      const res = await gql(`
        query AgroState {
          me { username role }
          registeredNodes { deviceId clientType deviceName petname isOnline lanAddress currentTrack }
          playbackHandoff(userId: "me") { trackTitle artistName albumName artworkUrl positionMs durationMs isPlaying deviceId }
        }
      `);
      const body = await res.json();
      const data = body?.data;
      if (data?.me) {
        setUsername(data.me.username);
        setRole(data.me.role || 'member');
      }
      if (data?.registeredNodes) {
        setNodes(data.registeredNodes);
      }
      if (data?.playbackHandoff) {
        setLastHandoff({
          title: data.playbackHandoff.trackTitle || 'Idle',
          artist: data.playbackHandoff.artistName || '',
          album: data.playbackHandoff.albumName || '',
          artworkUrl: data.playbackHandoff.artworkUrl || '',
          positionMs: data.playbackHandoff.positionMs || 0,
          durationMs: data.playbackHandoff.durationMs || 0,
          isPlaying: !!data.playbackHandoff.isPlaying,
          deviceId: data.playbackHandoff.deviceId || 'fleet'
        });
      }
      setLocked(false);
    } catch (err) {
      if (err.unauthorized) setLocked(true);
    }
  }, []);

  useEffect(() => {
    poll();
    const interval = setInterval(poll, 4000);
    return () => clearInterval(interval);
  }, [poll]);

  if (locked) {
    return (
      <AuthScreen
        ssoError={ssoError}
        onDismissSsoError={() => setSsoError('')}
        onSignedIn={() => {
          setLocked(false);
          window.location.reload();
        }}
      />
    );
  }

  const currentTabItem = NAV_ITEMS.find((item) => item.id === activeTab);

  return (
    <div className="app-shell">
      <Sidebar
        activeTab={activeTab}
        onTabSelect={handleTabSelect}
        username={username}
        isAdmin={isAdmin}
        unreadDrops={unreadDrops}
        onSignOut={handleSignOut}
      />

      <main className="main-area">
        <header className="page-header">
          <h1>{currentTabItem?.label ?? 'Agro'}</h1>
        </header>

        <div className="page-content">
          {activeTab === 'social' && (
            <SocialTab me={username} onUnauthorized={() => setLocked(true)} />
          )}

          {activeTab === 'devices' && (
            <DevicesTab
              username={username}
              nodes={nodes}
              onRenameNode={handleRenameNode}
              onDeleteNode={handleDeleteNode}
              onUnauthorized={() => setLocked(true)}
            />
          )}

          {activeTab === 'stats' && (
            <StatsTab username={username} nodes={nodes} onUnauthorized={() => setLocked(true)} />
          )}

          {activeTab === 'library' && (
            <LibraryBrowser username={username} devices={nodes} onUnauthorized={() => setLocked(true)} />
          )}

          {activeTab === 'links' && (
            <LinksTab username={username} onUnauthorized={() => setLocked(true)} />
          )}

          {activeTab === 'settings' && (
            <AccountSettingsTab username={username} onUnauthorized={() => setLocked(true)} />
          )}

          {activeTab === 'people' && isAdmin && (
            <PeopleTab me={username} onUnauthorized={() => setLocked(true)} />
          )}

          {activeTab === 'plugins' && isAdmin && (
            <AdminPluginsTab
              username={username}
              rules={rules}
              syncedSettings={syncedSettings}
              onUpdateSyncedSettings={setSyncedSettings}
              onToggleRule={handleToggleRule}
              onUnauthorized={() => setLocked(true)}
            />
          )}

          {activeTab === 'logs' && isAdmin && (
            <LogsTab logs={syncLogs} />
          )}
        </div>
      </main>

      <NowBar lastHandoff={lastHandoff} nodes={nodes} />
    </div>
  );
}
