import React, { useState } from 'react';
import {
  Activity,
  Server,
  Library,
  BarChart3,
  Settings,
  ShieldCheck,
  LogOut,
  ChevronDown
} from 'lucide-react';
import Avatar from '../Avatar.jsx';
import AgroLogo from './AgroLogo.jsx';

export const NAV_ITEMS = [
  { id: 'social', label: 'Social', icon: Activity, blurb: 'Friends, what they play, and the circle you share.' },
  { id: 'devices', label: 'Devices', icon: Server, blurb: 'Your players, the keys they sign in with, and their backups.' },
  { id: 'stats', label: 'Stats', icon: BarChart3, blurb: 'Your listening across every device, and what everyone here plays.' },
  { id: 'library', label: 'Library', icon: Library, blurb: 'Albums, playlists and where each track actually lives.' },
  { id: 'management', label: 'Management', icon: ShieldCheck, adminOnly: true, blurb: 'People, features and the health of this server.' }
];

export const ALL_TABS = [
  ...NAV_ITEMS,
  { id: 'settings', label: 'Settings', icon: Settings, blurb: 'Your profile, privacy, security and share links.' }
];

export default function Sidebar({
  activeTab,
  onTabSelect,
  username,
  isAdmin,
  unreadDrops = 0,
  onSignOut
}) {
  const [showUserMenu, setShowUserMenu] = useState(false);
  const visibleItems = NAV_ITEMS.filter((item) => !item.adminOnly || isAdmin);
  const activeIndex = visibleItems.findIndex((item) => item.id === activeTab);

  return (
    <aside className="sidebar">
      <div className="sidebar-brand">
        <AgroLogo size={30} />
        <span>Agro</span>
      </div>

      <nav className="sidebar-nav">
        {/* One pill for the whole list, sprung to the active row, so changing tab reads as motion. */}
        {activeIndex >= 0 && (
          <span className="nav-indicator" style={{ '--nav-index': activeIndex }} aria-hidden="true" />
        )}
        {visibleItems.map((item) => {
          const Icon = item.icon;
          const isActive = activeTab === item.id;
          return (
            <button
              key={item.id}
              className={`nav-item ${isActive ? 'active' : ''}`}
              onClick={() => onTabSelect(item.id)}
              aria-current={isActive ? 'page' : undefined}
            >
              <Icon size={18} />
              <span>{item.label}</span>
              {item.id === 'social' && unreadDrops > 0 && (
                <span className="nav-badge">{unreadDrops}</span>
              )}
            </button>
          );
        })}
      </nav>

      <div className="sidebar-footer">
        <div className="user-dropdown-container">
          <button
            type="button"
            className={`user-badge-btn ${activeTab === 'settings' ? 'active' : ''}`}
            onClick={() => setShowUserMenu(!showUserMenu)}
          >
            <Avatar username={username} size={22} />
            <span className="user-badge-name">{username || '…'}</span>
            {isAdmin && <span className="role-chip">admin</span>}
            <ChevronDown size={14} />
          </button>
          {showUserMenu && (
            <div className="user-dropdown-menu">
              <button
                type="button"
                className={`user-dropdown-action ${activeTab === 'settings' ? 'active' : ''}`}
                onClick={() => {
                  setShowUserMenu(false);
                  onTabSelect('settings');
                }}
              >
                <Settings size={15} />
                <span>Settings</span>
              </button>
              <div className="user-dropdown-divider" />
              <button
                type="button"
                className="user-dropdown-action danger"
                onClick={() => {
                  setShowUserMenu(false);
                  onSignOut();
                }}
              >
                <LogOut size={15} />
                <span>Sign out</span>
              </button>
            </div>
          )}
        </div>
        <div className="user-badge-version">{import.meta.env.VITE_AGRO_VERSION}</div>
      </div>
    </aside>
  );
}
