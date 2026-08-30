import React, { useCallback, useEffect, useState } from 'react';
import { QRCodeSVG } from 'qrcode.react';
import {
  Smartphone,
  Terminal,
  Trash2,
  Pencil,
  RefreshCw,
  Copy,
  Check,
  KeyRound,
  Plus
} from 'lucide-react';
import { gql } from '../api.js';

const APP_PASSWORDS_QUERY = `query Devices($user: String!) {
  appPasswords(userId: $user) { id label createdAt lastUsedAt }
}`;

const REVOKE_MUTATION = `mutation Revoke($user: String!, $id: Int!) {
  revokeAppPassword(userId: $user, id: $id)
}`;

function when(iso) {
  if (!iso) return 'never used';
  const seconds = Math.max(0, (Date.now() - new Date(iso).getTime()) / 1000);
  if (seconds < 90) return 'just now';
  if (seconds < 5400) return `${Math.round(seconds / 60)} min ago`;
  if (seconds < 172800) return `${Math.round(seconds / 3600)} h ago`;
  return `${Math.round(seconds / 86400)} d ago`;
}

export default function DevicesTab({ username, nodes = [], onRenameNode, onDeleteNode, onUnauthorized }) {
  const [appPasswords, setAppPasswords] = useState([]);
  const [busy, setBusy] = useState(null);
  const [notice, setNotice] = useState('');
  const [deviceNameInput, setDeviceNameInput] = useState('');
  const [pairing, setPairing] = useState(null);
  const [copied, setCopied] = useState(false);

  const load = useCallback(async () => {
    try {
      const res = await gql(APP_PASSWORDS_QUERY, { user: username });
      const body = await res.json();
      setAppPasswords(body?.data?.appPasswords ?? []);
    } catch (err) {
      if (err.unauthorized) onUnauthorized?.();
    }
  }, [username, onUnauthorized]);

  useEffect(() => { load(); }, [load]);

  const handlePair = async () => {
    const label = deviceNameInput.trim();
    if (!label) return;
    try {
      const res = await gql(
        `mutation MintToken($user: String!, $label: String!) {
          createAppPassword(userId: $user, label: $label) { token label }
        }`,
        { user: username, label }
      );
      const body = await res.json();
      if (body?.errors?.length) throw new Error(body.errors[0].message);
      setPairing(body?.data?.createAppPassword);
      setDeviceNameInput('');
      load();
    } catch (e) {
      if (e.unauthorized) onUnauthorized?.();
      else alert(e.message);
    }
  };

  const handleCopy = () => {
    if (!pairing?.token) return;
    navigator.clipboard.writeText(pairing.token);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  async function revoke(device) {
    const ok = window.confirm(
      `Revoke “${device.label}”?\n\nThat device will be signed out immediately.`
    );
    if (!ok) return;

    setBusy(device.id);
    setNotice('');
    try {
      const res = await gql(REVOKE_MUTATION, { user: username, id: device.id });
      const body = await res.json();
      if (body?.errors?.length) throw new Error(body.errors[0].message);
      setNotice(`Revoked “${device.label}”.`);
      await load();
    } catch (err) {
      if (err.unauthorized) onUnauthorized?.();
      else setNotice(err.message);
    } finally {
      setBusy(null);
    }
  }

  const serverHost = window.location.origin;
  const qrPayload = pairing ? JSON.stringify({
    server: serverHost,
    username,
    token: pairing.token
  }) : '';

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: '20px' }}>
      {/* Section 1: Active Fleet Nodes */}
      <div className="card">
        <div className="card-header">
          <div>
            <div className="card-title">Connected Fleet Devices ({nodes.length})</div>
            <div className="card-subtitle">Wander and Wanda instances reporting live state</div>
          </div>
        </div>

        {nodes.length === 0 ? (
          <div className="empty-hint" style={{ padding: '24px', textAlign: 'center' }}>
            No devices currently registered. Launch <strong>Wander</strong> or <strong>Wanda</strong> to connect.
          </div>
        ) : (
          <div className="nodes-grid">
            {nodes.map(node => (
              <div key={node.deviceId} className="node-card">
                <div className="node-header">
                  <div>
                    <div className="node-name">
                      {node.clientType.toLowerCase().includes('wanda') ? <Smartphone size={14} /> : <Terminal size={14} />}
                      <span>{node.petname}</span>
                    </div>
                    <div className="node-type">{node.clientType.toLowerCase()}</div>
                  </div>
                  <div style={{ display: 'flex', alignItems: 'center', gap: '6px' }}>
                    <span className="daemon-pill" style={{
                      fontSize: '10px',
                      padding: '2px 6px',
                      color: node.isOnline ? 'var(--status-active)' : 'var(--text-muted)'
                    }}>
                      {node.isOnline ? 'ONLINE' : 'AWAY'}
                    </span>
                    <button
                      onClick={() => onRenameNode?.(node)}
                      title="Rename device"
                      className="icon-btn"
                    >
                      <Pencil size={13} />
                    </button>
                    <button
                      onClick={() => onDeleteNode?.(node.deviceId)}
                      title="Remove device"
                      className="icon-btn"
                    >
                      <Trash2 size={13} />
                    </button>
                  </div>
                </div>
                <div className="node-footer">
                  <span>{node.currentTrack ? `Track: ${node.currentTrack}` : 'Status: Idle'}</span>
                </div>
              </div>
            ))}
          </div>
        )}
      </div>

      {/* Section 2: Pair a New Device */}
      <div className="card">
        <div className="card-header">
          <div>
            <div className="card-title">Pair a Device</div>
            <div className="card-subtitle">Generate a scoped token and QR code for Wander or Wanda</div>
          </div>
          <div className="pair-issue">
            <input
              type="text"
              placeholder="e.g. Living room laptop"
              value={deviceNameInput}
              onChange={(e) => setDeviceNameInput(e.target.value)}
              onKeyDown={(e) => { if (e.key === 'Enter') handlePair(); }}
            />
            <button
              className="btn btn-secondary"
              onClick={handlePair}
              disabled={!deviceNameInput.trim()}
            >
              <Plus size={14} />
              <span>Issue Token</span>
            </button>
          </div>
        </div>

        {pairing && (
          <div className="pairing-container">
            <div className="qr-box">
              <QRCodeSVG value={qrPayload} size={140} level="M" />
            </div>

            <div style={{ width: '100%', display: 'flex', flexDirection: 'column', alignItems: 'center', gap: '6px' }}>
              <div style={{ fontSize: '11px', color: 'var(--text-muted)', textTransform: 'uppercase', letterSpacing: '0.05em' }}>
                Device Token — Shown Once
              </div>
              <div className="passphrase-display">
                <span style={{ wordBreak: 'break-all' }}>{pairing.token}</span>
                <button className="btn btn-secondary" onClick={handleCopy} style={{ padding: '4px 8px' }}>
                  {copied ? <Check size={13} color="var(--status-active)" /> : <Copy size={13} />}
                </button>
              </div>
            </div>

            <div className="code-snippet">
              <div style={{ color: 'var(--text-muted)', marginBottom: '4px' }}># ~/.config/wander/config.toml</div>
              <div>[agro]</div>
              <div>enabled = true</div>
              <div>server = "{serverHost}"</div>
              <div>username = "{username}"</div>
              <div>token = "{pairing.token}"</div>
            </div>
          </div>
        )}
      </div>

      {/* Section 3: Active Sign-ins / App Passwords */}
      <div className="card">
        <div className="card-header">
          <div>
            <div className="card-title">Active Sign-ins & App Passwords</div>
            <div className="card-subtitle">Active authentication credentials issued for your account</div>
          </div>
          <button className="btn btn-secondary" onClick={load}>
            <RefreshCw size={13} />
            <span>Refresh</span>
          </button>
        </div>

        {notice && <div className="empty-hint" style={{ padding: '10px 16px' }}>{notice}</div>}

        {appPasswords.length === 0 ? (
          <div className="empty-hint" style={{ padding: '20px' }}>
            No tokens registered.
          </div>
        ) : (
          <div className="rules-list">
            {appPasswords.map((device) => (
              <div key={device.id} className="rule-row">
                <div className="rule-info">
                  <div className="rule-title">
                    <KeyRound size={13} style={{ marginRight: '6px', verticalAlign: '-2px' }} />
                    {device.label}
                  </div>
                  <div className="rule-desc">
                    Created {new Date(device.createdAt).toLocaleDateString()} · {when(device.lastUsedAt)}
                  </div>
                </div>
                <button
                  className="btn btn-secondary"
                  disabled={busy === device.id}
                  onClick={() => revoke(device)}
                >
                  <Trash2 size={13} />
                  <span>{busy === device.id ? 'Revoking…' : 'Revoke'}</span>
                </button>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
