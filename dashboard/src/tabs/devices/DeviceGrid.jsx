import { useState } from 'react';
import { QRCodeSVG } from 'qrcode.react';
import { Check, Copy, Music2, Pencil, Plus, Smartphone, Terminal, Trash2, X } from 'lucide-react';
import Cookie from '../../expressive/Cookie.jsx';

/**
 * Every player on the account as a tile — a phone on a cookie, a desktop on a clover — and, as the
 * last tile, the way to add one. Pairing mints a token under the name typed, then shows it once as
 * a QR code for Wanda and a config snippet for Wander.
 */
export default function DeviceGrid({ nodes, username, onRename, onDelete, onPair }) {
  const [naming, setNaming] = useState(false);
  const [name, setName] = useState('');
  const [pairing, setPairing] = useState(null);

  async function pair() {
    const label = name.trim();
    if (!label) return;
    const minted = await onPair(label);
    if (minted) {
      setPairing(minted);
      setName('');
      setNaming(false);
    }
  }

  return (
    <>
      <div className="device-grid">
        {nodes.map(node => {
          const phone = node.clientType.toLowerCase().includes('wanda');
          return (
            <div key={node.deviceId} className={`device-tile ${node.isOnline ? 'is-online' : ''}`}>
              <div className="device-tile-top">
                <Cookie shape={phone ? 'cookie' : 'clover'} size={64} className="device-shape">
                  {phone ? <Smartphone size={26} /> : <Terminal size={26} />}
                </Cookie>
                <div className="device-tile-actions">
                  <button className="icon-btn" onClick={() => onRename(node)} aria-label="Rename"><Pencil size={15} /></button>
                  <button className="icon-btn danger" onClick={() => onDelete(node.deviceId)} aria-label="Remove"><Trash2 size={15} /></button>
                </div>
              </div>
              <div className="device-name">{node.petname}</div>
              <div className="device-kind">
                <span className="device-dot" />
                {phone ? 'Wanda' : 'Wander'} · {node.isOnline ? 'online' : 'away'}
              </div>
              {node.currentTrack && (
                <div className="device-track"><Music2 size={14} /><span>{node.currentTrack}</span></div>
              )}
            </div>
          );
        })}

        <div className={`device-tile device-pair ${naming ? 'is-open' : ''}`}>
          {naming ? (
            <form className="device-pair-form" onSubmit={e => { e.preventDefault(); pair(); }}>
              <span className="device-name">Name it</span>
              <input autoFocus placeholder="Living room laptop" value={name} onChange={e => setName(e.target.value)} />
              <div className="device-pair-buttons">
                <button type="submit" className="btn btn-primary" disabled={!name.trim()}>Pair</button>
                <button type="button" className="icon-btn" onClick={() => setNaming(false)} aria-label="Cancel"><X size={16} /></button>
              </div>
            </form>
          ) : (
            <button className="device-pair-open" onClick={() => setNaming(true)}>
              <Cookie shape="sunny" size={64} className="device-shape"><Plus size={28} /></Cookie>
              <span className="device-name">Pair a device</span>
            </button>
          )}
        </div>
      </div>

      {pairing && <PairingCard pairing={pairing} username={username} onClose={() => setPairing(null)} />}
    </>
  );
}

/** The new token, shown once: a QR code for Wanda, a snippet for Wander. */
function PairingCard({ pairing, username, onClose }) {
  const [copied, setCopied] = useState(false);
  const server = window.location.origin;
  const qr = `agro://connect?username=${encodeURIComponent(username)}&token=${encodeURIComponent(pairing.token)}&server=${encodeURIComponent(server)}`;

  function copy() {
    navigator.clipboard.writeText(pairing.token);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  }

  return (
    <div className="card pairing-card">
      <div className="pairing-qr"><QRCodeSVG value={qr} size={168} level="M" /></div>
      <div className="pairing-body">
        <span className="stats-overline">{pairing.label} · shown once</span>
        <span className="pairing-title">Scan with Wanda</span>
        <div className="passphrase-display">
          <span>{pairing.token}</span>
          <button className="icon-btn" onClick={copy} aria-label="Copy token">
            {copied ? <Check size={15} /> : <Copy size={15} />}
          </button>
        </div>
        <pre className="code-snippet">{`# ~/.config/wander/config.toml
[agro]
enabled = true
server = "${server}"
username = "${username}"
device_token = "${pairing.token}"`}</pre>
      </div>
      <button className="icon-btn pairing-close" onClick={onClose} aria-label="Done"><X size={16} /></button>
    </div>
  );
}
