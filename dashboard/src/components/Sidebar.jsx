import React, { useState } from 'react';
import {
  Activity,
  Server,
  Library,
  BarChart3,
  Link2 as LinkIcon,
  Settings,
  UserPlus,
  Layers,
  ScrollText,
  ChevronDown
} from 'lucide-react';
import Avatar from '../Avatar.jsx';

export const NAV_ITEMS = [
  { id: 'social', label: 'Social', icon: Activity },
  { id: 'devices', label: 'Devices & Sign-ins', icon: Server },
  { id: 'stats', label: 'Stats', icon: BarChart3 },
  { id: 'library', label: 'Library', icon: Library },
  { id: 'links', label: 'Links', icon: LinkIcon },
  { id: 'settings', label: 'Settings', icon: Settings },
  { id: 'people', label: 'People', icon: UserPlus, adminOnly: true },
  { id: 'plugins', label: 'Plugins', icon: Layers, adminOnly: true },
  { id: 'logs', label: 'Logs', icon: ScrollText, adminOnly: true }
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
      <div className="sidebar-brand">Agro</div>

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
            className="user-badge-btn"
            onClick={() => setShowUserMenu(!showUserMenu)}
          >
            <Avatar username={username} size={22} />
            <span className="user-badge-name">{username || '…'}</span>
            {isAdmin && <span className="role-chip">admin</span>}
            <ChevronDown size={14} />
          </button>
          {showUserMenu && (
            <div className="user-dropdown-menu">
              <div className="user-dropdown-row">
                <button
                  className="btn btn-secondary btn-block"
                  onClick={() => {
                    setShowUserMenu(false);
                    onSignOut();
                  }}
                >
                  Sign out
                </button>
              </div>
            </div>
          )}
        </div>
      </div>
    </aside>
  );
}
