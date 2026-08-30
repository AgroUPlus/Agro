import React, { useState, useEffect, useCallback } from 'react';
import {
  getToken,
  setToken,
  gql,
  consumeSsoFragment,
  setEnrolmentRequiredHandler,
  FALLBACK_RULES
} from './api.js';
import Sidebar, { NAV_ITEMS } from './components/Sidebar.jsx';
import NowBar from './components/NowBar.jsx';
import AuthScreen from './AuthScreen.jsx';
import EnrolTotpScreen from './EnrolTotpScreen.jsx';

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
  /**
   * Raised by any query the server refuses until a second factor exists. Registered once, because
   * the refusal arrives on every query at once rather than on one screen.
   */
  const [needsEnrolment, setNeedsEnrolment] = useState(false);

  useEffect(() => {
    setEnrolmentRequiredHandler(() => setNeedsEnrolment(true));
  }, []);
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
      // Two round trips rather than one, because both of the other fields are scoped to an
      // account and the server will not accept a stand-in for it. This document used to ask for
      // `registeredNodes`, which is not a field on Query, and for `playbackHandoff(userId: "me")`,
      // where "me" was compared literally against the caller's username and refused. An unknown
      // field is a *validation* error, so the whole document was rejected before anything ran and
      // `me` never resolved -- which is why the dashboard rendered with no username, no devices
      // and empty settings.
      const meRes = await gql(`query Me { me { username role } }`);
      const meData = (await meRes.json())?.data;
      if (!meData?.me) return;

      const who = meData.me.username;
      setUsername(who);
      setRole(meData.me.role || 'member');

      const res = await gql(
        `query AgroState($who: String!) {
          activeNodes(userId: $who) { deviceId clientType petname isOnline lanAddress currentTrack }
          playbackHandoff(userId: $who) { trackTitle artistName albumName artworkUrl positionMs durationMs isPlaying deviceId }
        }`,
        { who }
      );
      const body = await res.json();
      const data = body?.data;
      if (data?.activeNodes) {
        setNodes(data.activeNodes);
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

  // Checked before `locked`, and before any tab renders: the account is authenticated but may do
  // nothing until it enrols, so every other screen would be empty.
  if (needsEnrolment && !locked) {
    return <EnrolTotpScreen onEnrolled={() => window.location.reload()} />;
  }

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
