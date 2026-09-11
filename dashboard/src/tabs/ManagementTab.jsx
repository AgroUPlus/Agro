import React, { useState } from 'react';
import { Users, Layers, ScrollText } from 'lucide-react';
import PeopleTab from './PeopleTab.jsx';
import AdminPluginsTab from './AdminPluginsTab.jsx';
import LogsTab from './LogsTab.jsx';

export default function ManagementTab({
  me,
  rules,
  onToggleRule,
  logs = [],
  onUnauthorized
}) {
  const [subSection, setSubSection] = useState('people'); // 'people' | 'plugins' | 'logs'

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: '16px' }}>
      {/* Management Sub-section Switcher */}
      <div className="card" style={{ padding: '8px 16px' }}>
        <div className="segmented">
          <button
            type="button"
            className={`segmented-btn ${subSection === 'people' ? 'active' : ''}`}
            onClick={() => setSubSection('people')}
          >
            <Users size={14} style={{ marginRight: '6px' }} />
            People & Accounts
          </button>
          <button
            type="button"
            className={`segmented-btn ${subSection === 'plugins' ? 'active' : ''}`}
            onClick={() => setSubSection('plugins')}
          >
            <Layers size={14} style={{ marginRight: '6px' }} />
            Plugins & Rules
          </button>
          <button
            type="button"
            className={`segmented-btn ${subSection === 'logs' ? 'active' : ''}`}
            onClick={() => setSubSection('logs')}
          >
            <ScrollText size={14} style={{ marginRight: '6px' }} />
            Server Logs
          </button>
        </div>
      </div>

      {subSection === 'people' && (
        <PeopleTab me={me} onUnauthorized={onUnauthorized} />
      )}
      {subSection === 'plugins' && (
        <AdminPluginsTab rules={rules} onToggleRule={onToggleRule} />
      )}
      {subSection === 'logs' && (
        <LogsTab logs={logs} />
      )}
    </div>
  );
}
