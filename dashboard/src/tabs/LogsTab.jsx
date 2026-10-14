import { ScrollText } from 'lucide-react';
import Cookie from '../expressive/Cookie.jsx';

/** What this dashboard has done to the fleet since it was opened, newest first. */
export default function LogsTab({ logs = [] }) {
  if (logs.length === 0) {
    return (
      <div className="empty-state">
        <Cookie shape="clover" size={88}><ScrollText size={34} /></Cookie>
        <span className="empty-state-title">Nothing logged yet</span>
        <span className="empty-state-sub">Changes made from this page show up here.</span>
      </div>
    );
  }

  return (
    <div className="card">
      <div className="terminal-card">
        {logs.map((log, idx) => (
          <div key={idx} className="terminal-line">
            <span className="terminal-ts">{log.time}</span>
            <span>{log.event}</span>
          </div>
        ))}
      </div>
    </div>
  );
}
