import { Download, KeyRound, Lock, Smartphone, Trash2 } from 'lucide-react';
import { formatBytes } from '../../api.js';
import Cookie from '../../expressive/Cookie.jsx';
import { ago, sectionMeta } from './sections.js';

/** The newest backup, large: when, from where, and what is inside, as icons with counts. */
export function BackupHero({ backup, busy, onDownload, onDelete }) {
  return (
    <div className="card backup-hero">
      <div className="backup-hero-head">
        <Cookie shape="cookie" size={76} className="backup-hero-shape">
          <Lock size={30} />
        </Cookie>
        <div className="backup-hero-text">
          <span className="stats-overline">Latest backup</span>
          <span className="backup-hero-when">{ago(backup.createdAt)}</span>
          <span className="backup-hero-meta">
            <Smartphone size={14} />
            {backup.deviceName || backup.deviceId}
            {backup.appVersion && <> · Wanda {backup.appVersion}</>}
            {' · '}{formatBytes(backup.sealedBytes)}
          </span>
        </div>
        <div className="backup-actions">
          <button className="btn btn-primary" disabled={busy} onClick={() => onDownload(backup)}>
            <Download size={15} />Download
          </button>
          <button className="icon-btn danger" disabled={busy} onClick={() => onDelete(backup)} aria-label="Delete">
            <Trash2 size={16} />
          </button>
        </div>
      </div>

      <div className="backup-sections">
        {backup.sections.map(section => {
          const { label, icon: Icon } = sectionMeta(section.name);
          return (
            <div key={section.name} className={`backup-section ${section.name === 'ACCOUNTS' ? 'is-sensitive' : ''}`}>
              <Icon size={20} />
              <span className="backup-section-count">{section.count.toLocaleString()}</span>
              <span className="backup-section-label">{label}</span>
            </div>
          );
        })}
      </div>

      {backup.includesAccounts && (
        <p className="backup-note">
          <KeyRound size={14} /> Holds sign-ins, sealed like the rest. Your passphrase is what keeps them safe.
        </p>
      )}
    </div>
  );
}

/** How many of the kept slots are used, as pips, and the one thing to know about all of them. */
export function BackupSlots({ used, limits }) {
  const slots = limits?.keepPerAccount ?? used;
  return (
    <div className="card backup-slots">
      <div className="backup-pips" aria-label={`${used} of ${slots} kept`}>
        {Array.from({ length: slots }, (_, index) => (
          <Cookie key={index} shape="sunny" size={34} className={index < used ? 'pip-on' : 'pip-off'} />
        ))}
      </div>
      <span className="backup-slots-label">{used} of {slots} kept</span>
      <span className="backup-slots-sub">
        <Lock size={13} /> Sealed on your phone. This server can't open them.
        {limits && <> Up to {formatBytes(limits.maxSealedBytes)} each.</>}
      </span>
    </div>
  );
}

/** An older backup, on one line. */
export function BackupRow({ backup, busy, onDownload, onDelete }) {
  return (
    <li className="backup-row">
      <Smartphone size={18} />
      <span className="backup-row-text">
        <span className="backup-row-when">{ago(backup.createdAt)}</span>
        <span className="backup-row-sub">{backup.deviceName || backup.deviceId} · {formatBytes(backup.sealedBytes)}</span>
      </span>
      <span className="backup-row-icons">
        {backup.sections.map(section => {
          const { label, icon: Icon } = sectionMeta(section.name);
          return <Icon key={section.name} size={15} aria-label={label} />;
        })}
      </span>
      <button className="icon-btn" disabled={busy} onClick={() => onDownload(backup)} aria-label="Download">
        <Download size={16} />
      </button>
      <button className="icon-btn danger" disabled={busy} onClick={() => onDelete(backup)} aria-label="Delete">
        <Trash2 size={16} />
      </button>
    </li>
  );
}
