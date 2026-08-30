import React from 'react';
import { QRCodeSVG } from 'qrcode.react';
import { Shield, Check, KeyRound } from 'lucide-react';

export default function SecuritySection({
  hasTotp,
  totpEnrolment,
  totpCode,
  onTotpCodeChange,
  totpNotice,
  onBeginTotp,
  onConfirmTotp,
  onDisableTotp,
  recoveryCodes,
  onDismissRecoveryCodes,
  disableCode,
  onDisableCodeChange,
  onRegenerateRecovery
}) {
  return (
    <div className="card">
      <div className="card-header">
        <div>
          <div className="card-title">Security & Two-Factor Authentication (2FA)</div>
          <div className="card-subtitle">RFC 6238 TOTP authenticator protection for your account</div>
        </div>
        {hasTotp ? null : (
          <button className="btn btn-secondary" onClick={onBeginTotp} disabled={!!totpEnrolment}>
            <Shield size={14} />
            <span>Setup 2FA</span>
          </button>
        )}
      </div>

      {totpNotice && <div className="empty-hint" style={{ padding: '10px 14px' }}>{totpNotice}</div>}

      {recoveryCodes && (
        <div style={{ marginTop: '10px', background: 'var(--bg-surface-elevated)', padding: '16px', borderRadius: 'var(--radius-sm)' }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: '8px', marginBottom: '8px' }}>
            <KeyRound size={14} />
            <strong style={{ fontSize: '13px' }}>Save your recovery codes</strong>
          </div>
          <div style={{ fontSize: '12px', color: 'var(--text-muted)', marginBottom: '10px' }}>
            These are shown <strong>once</strong>. The server keeps only digests of them and cannot
            show them again. Each one works a single time, and any of them can be used in place of a
            code if you lose your authenticator — without them, a lost phone means a lost account.
          </div>
          <div style={{ display: 'grid', gridTemplateColumns: 'repeat(2, 1fr)', gap: '6px' }}>
            {recoveryCodes.map((code) => (
              <code key={code} style={{ fontSize: '13px', background: 'var(--bg-surface)', padding: '6px 8px', borderRadius: '4px' }}>
                {code}
              </code>
            ))}
          </div>
          <div style={{ display: 'flex', gap: '8px', marginTop: '12px' }}>
            <button
              className="btn btn-secondary"
              onClick={() => navigator.clipboard?.writeText(recoveryCodes.join('\n'))}
            >
              Copy all
            </button>
            <button className="btn btn-primary" onClick={onDismissRecoveryCodes}>
              I have saved them
            </button>
          </div>
        </div>
      )}

      {hasTotp ? (
        <div style={{ display: 'flex', flexDirection: 'column', gap: '12px', padding: '12px 0' }}>
          <div className="badge-chip success" style={{ display: 'flex', alignItems: 'center', gap: '6px', alignSelf: 'flex-start' }}>
            <Check size={13} />
            <span>Two-factor authentication is on</span>
          </div>
          <div style={{ fontSize: '12px', color: 'var(--text-muted)' }}>
            Turning this off, or issuing new recovery codes, needs a current code — a recovery code
            works here too. Without that check, a stolen session could simply remove the protection
            it never had to pass.
          </div>
          <div style={{ display: 'flex', gap: '8px', flexWrap: 'wrap' }}>
            <input
              type="text"
              placeholder="Current code"
              value={disableCode}
              onChange={(e) => onDisableCodeChange(e.target.value)}
              style={{ width: '150px', padding: '6px 8px' }}
            />
            <button className="btn btn-secondary" onClick={onRegenerateRecovery}>
              New recovery codes
            </button>
            <button className="btn btn-secondary" onClick={onDisableTotp}>
              Turn off 2FA
            </button>
          </div>
        </div>
      ) : totpEnrolment ? (
        <div style={{ display: 'flex', flexDirection: 'column', gap: '12px', marginTop: '10px', background: 'var(--bg-surface-elevated)', padding: '16px', borderRadius: 'var(--radius-sm)' }}>
          <div style={{ fontSize: '12px', color: 'var(--text-main)' }}>
            Scan the QR code with your authenticator app (Google Authenticator, Aegis, 1Password):
          </div>
          <div style={{ display: 'flex', gap: '20px', alignItems: 'center' }}>
            <div style={{ background: '#fff', padding: '8px', borderRadius: '6px' }}>
              <QRCodeSVG value={totpEnrolment.otpauthUri} size={140} level="M" />
            </div>
            <div style={{ display: 'flex', flexDirection: 'column', gap: '8px' }}>
              <div style={{ fontSize: '11px', color: 'var(--text-muted)' }}>Manual entry secret:</div>
              <code style={{ fontSize: '13px', background: 'var(--bg-surface)', padding: '6px 8px', borderRadius: '4px' }}>
                {totpEnrolment.secretBase32}
              </code>
              <div style={{ display: 'flex', gap: '8px', marginTop: '6px' }}>
                <input
                  type="text"
                  placeholder="6-digit code"
                  maxLength={6}
                  value={totpCode}
                  onChange={(e) => onTotpCodeChange(e.target.value)}
                  style={{ width: '120px', padding: '6px 8px' }}
                />
                <button className="btn btn-primary" onClick={onConfirmTotp} disabled={totpCode.length !== 6}>
                  Verify & Enable
                </button>
              </div>
            </div>
          </div>
        </div>
      ) : (
        <div style={{ fontSize: '12px', color: 'var(--text-muted)', padding: '8px 0' }}>
          Add an extra layer of security requiring a 6-digit one-time code when signing in.
        </div>
      )}
    </div>
  );
}
