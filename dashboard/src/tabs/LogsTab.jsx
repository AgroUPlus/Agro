import React from 'react';

export default function LogsTab({ logs = [] }) {
  return (
    <div className="card">
      <div className="card-header">
        <div>
          <div className="card-title">Server Security & Sync Logs</div>
          <div className="card-subtitle">Real-time system events, token sweeps, and relay transfers</div>
        </div>
      </div>

      <div className="terminal-card">
        {logs.length === 0 ? (
          <div style={{ color: 'var(--text-muted)', padding: '12px' }}>No log entries recorded.</div>
        ) : (
          logs.map((log, idx) => (
            <div key={idx} className="terminal-line">
              <span className="terminal-ts">[{log.time}]</span>
              <span>{log.event}</span>
            </div>
          ))
        )}
      </div>
    </div>
  );
}
