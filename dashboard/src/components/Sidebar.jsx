import React, { useState } from 'react';
import {
  Activity,
  Server,
  Library,
  BarChart3,
  TrendingUp,
  Link2 as LinkIcon,
  Settings,
  ShieldCheck,
  LogOut,
  ChevronDown
} from 'lucide-react';
import Avatar from '../Avatar.jsx';
import AgroLogo from './AgroLogo.jsx';

export const NAV_ITEMS = [
  { id: 'social', label: 'Social', icon: Activity },
  { id: 'devices', label: 'Devices & Sign-ins', icon: Server },
  { id: 'stats', label: 'Stats', icon: BarChart3 },
  { id: 'popular', label: 'Popular on Agro', icon: TrendingUp },
  { id: 'library', label: 'Library', icon: Library },
  { id: 'links', label: 'Links', icon: LinkIcon },
  { id: 'management', label: 'Management', icon: ShieldCheck, adminOnly: true }
];

export const ALL_TABS = [
  ...NAV_ITEMS,
  { id: 'settings', label: 'Settings', icon: Settings }
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

  return (
    <aside className="sidebar">
      <div className="sidebar-brand">
        <AgroLogo size={30} />
        <span>Agro</span>
      </div>

      <nav className="sidebar-nav">
        {NAV_ITEMS.filter((item) => !item.adminOnly || isAdmin).map((item) => {
          const Icon = item.icon;
          const isActive = activeTab === item.id;
          return (
            <button
              key={item.id}
              className={`nav-item ${isActive ? 'active' : ''}`}
              onClick={() => onTabSelect(item.id)}
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
      </div>
    </aside>
  );
}
