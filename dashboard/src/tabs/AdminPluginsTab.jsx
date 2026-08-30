import React from 'react';
import { FALLBACK_RULES } from '../api.js';

/**
 * Sync rules and plugin state.
 *
 * The "Smart Share Links Domain" card used to live here and has moved to Settings, where the rest
 * of the synced preferences are. It was in the wrong place and did not work:
 *
 * - `synced_settings` is keyed by user id, so the share domain is a *per-account* preference. This
 *   tab is admin-only, which meant nobody else could set their own.
 * - Its values were never loaded. They came from a hardcoded default in `App`, so the inputs
 *   showed placeholders no matter what the account actually had stored.
 * - Its mutation still sent `serverUrl`, `serverUsername` and `lrclibUrl`, which migration 27
 *   removed from the input type, so every save failed validation. It reported success anyway,
 *   because it checked the HTTP status rather than the GraphQL errors -- a 200 carrying an error
 *   body read as "Saved!".
 * - Had those fields still existed, it would have written its hardcoded `http://localhost:4533`
 *   and `alpha` over the real settings the moment anyone pressed the button.
 */
export default function AdminPluginsTab({
  rules = FALLBACK_RULES,
  onToggleRule
}) {
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: '16px' }}>
      {/* Sync Rules */}
      <div className="card">
        <div className="card-header">
          <div className="card-title">Plugins & Core Sync Rules</div>
        </div>

        <div className="rules-list">
          {rules.map(rule => (
            <div key={rule.id} className={`rule-row ${!rule.isEnabled ? 'disabled' : ''}`}>
              <div className="rule-info">
                <div className="rule-title">
                  {rule.name}
                  <span className="rule-tag">{rule.target}</span>
                </div>
                <div className="rule-desc">{rule.description}</div>
              </div>

              <label className="switch">
                <input
                  type="checkbox"
                  checked={rule.isEnabled}
                  onChange={() => onToggleRule(rule.id)}
                />
                <span className="slider" />
              </label>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
