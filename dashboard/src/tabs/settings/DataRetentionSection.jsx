import React from 'react';
import { Trash2 } from 'lucide-react';

export default function DataRetentionSection({
  purgeYear,
  onPurgeYearChange,
  onPurgeScrobbles,
  purgeNotice
}) {
  return (
    <div className="card">
      <div className="card-header">
        <div>
          <div className="card-title">Your history</div>
          <div className="card-subtitle">Delete listening history from this server.</div>
        </div>
      </div>

      {purgeNotice && <div className="empty-hint" style={{ padding: '10px 14px' }}>{purgeNotice}</div>}

      <div style={{ display: 'flex', gap: '12px', alignItems: 'center', marginTop: '6px' }}>
        <select
          className="form-input"
          style={{ width: '180px' }}
          value={purgeYear}
          onChange={(e) => onPurgeYearChange(e.target.value)}
        >
          <option value="">All-Time History</option>
          <option value="2026">Year 2026</option>
          <option value="2025">Year 2025</option>
          <option value="2024">Year 2024</option>
        </select>

        <button className="btn btn-secondary" onClick={onPurgeScrobbles}>
          <Trash2 size={13} color="var(--status-danger)" />
          <span>Delete history</span>
        </button>
      </div>
    </div>
  );
}
