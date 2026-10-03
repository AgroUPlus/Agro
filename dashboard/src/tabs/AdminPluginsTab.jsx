import React, { useCallback, useEffect, useState } from 'react';
import { gql } from '../api.js';

/**
 * What this server does, and the switches for the parts that cost something.
 *
 * Read from the server's own `plugins` list. This page used to render a hardcoded list whose ids
 * the server had never heard of, so every switch on it wrote a row nothing read: it looked like
 * control and changed nothing. Now each switchable entry is a feature the server refuses while it
 * is off, and the rest are shown as status only, because there is nothing to turn off.
 */
const PLUGINS_QUERY = `{
  plugins { id name description category target isEnabled isConnected toggleable endpoint
            metadata { key value } }
}`;

const TOGGLE = `mutation Toggle($id: String!, $on: Boolean!) {
  togglePlugin(pluginId: $id, isEnabled: $on)
}`;

/** Order the groups by what an operator on a small box is likeliest to want to cut first. */
const CATEGORY_ORDER = ['Bandwidth', 'Social', 'Discovery', 'Sharing'];

function groupByCategory(features) {
  const groups = new Map();
  for (const feature of features) {
    if (!groups.has(feature.category)) groups.set(feature.category, []);
    groups.get(feature.category).push(feature);
  }
  const rank = (category) => {
    const index = CATEGORY_ORDER.indexOf(category);
    return index === -1 ? CATEGORY_ORDER.length : index;
  };
  return [...groups.entries()].sort(([a], [b]) => rank(a) - rank(b));
}

function Metadata({ items }) {
  return items.map((m) => (
    <div key={m.key} className="rule-desc" style={{ opacity: 0.75 }}>
      <strong>{m.key}:</strong> {m.value}
    </div>
  ));
}

export default function AdminPluginsTab({ onUnauthorized }) {
  const [plugins, setPlugins] = useState(null);
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(null);

  const load = useCallback(async () => {
    try {
      const body = await (await gql(PLUGINS_QUERY)).json();
      if (body?.errors?.length) {
        setError(body.errors[0].message);
        return;
      }
      setPlugins(body?.data?.plugins ?? []);
    } catch (e) {
      if (e.unauthorized) onUnauthorized?.();
      else setError('Could not reach the server.');
    }
  }, [onUnauthorized]);

  useEffect(() => {
    load();
  }, [load]);

  // Not optimistic: the switch moves when the server has agreed, so it never shows a state the
  // server is not in.
  async function toggle(plugin) {
    const next = !plugin.isEnabled;
    setBusy(plugin.id);
    setError('');
    try {
      const body = await (await gql(TOGGLE, { id: plugin.id, on: next })).json();
      if (body?.errors?.length) {
        setError(body.errors[0].message);
        return;
      }
      setPlugins((prev) => prev.map((p) => (p.id === plugin.id ? { ...p, isEnabled: next } : p)));
    } catch (e) {
      if (e.unauthorized) onUnauthorized?.();
      else setError('Could not reach the server; nothing was changed.');
    } finally {
      setBusy(null);
    }
  }

  if (plugins === null) {
    return (
      <div className="card">
        {error
          ? <p style={{ color: 'var(--danger, #f66)' }}>{error}</p>
          : <p style={{ opacity: 0.6 }}>Loading…</p>}
      </div>
    );
  }

  const features = plugins.filter((p) => p.toggleable);
  const connectors = plugins.filter((p) => !p.toggleable);

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: '16px' }}>
      {error && <div className="card" style={{ color: 'var(--danger, #f66)' }}>{error}</div>}

      <div className="card">
        <div className="card-header">
          <div className="card-title">Server features</div>
        </div>
        <p style={{ opacity: 0.7, marginTop: 0 }}>
          Switch off what this machine cannot afford. A feature that is off is refused for every
          account, and clients are told so rather than left retrying. Nothing is deleted:
          switching it back on picks up where it left off.
        </p>

        {groupByCategory(features).map(([category, group]) => (
          <div key={category} style={{ marginTop: '12px' }}>
            <div className="rule-tag" style={{ display: 'inline-block', marginBottom: '6px' }}>
              {category}
            </div>
            <div className="rules-list">
              {group.map((feature) => (
                <div
                  key={feature.id}
                  className={`rule-row ${!feature.isEnabled ? 'disabled' : ''}`}
                >
                  <div className="rule-info">
                    <div className="rule-title">{feature.name}</div>
                    <div className="rule-desc">{feature.description}</div>
                    <Metadata items={feature.metadata} />
                  </div>
                  <label className="switch">
                    <input
                      type="checkbox"
                      aria-label={`${feature.name}: ${feature.isEnabled ? 'on' : 'off'}`}
                      checked={feature.isEnabled}
                      disabled={busy === feature.id}
                      onChange={() => toggle(feature)}
                    />
                    <span className="slider" />
                  </label>
                </div>
              ))}
            </div>
          </div>
        ))}
      </div>

      <div className="card">
        <div className="card-header">
          <div className="card-title">Clients & connectors</div>
        </div>
        <div className="rules-list">
          {connectors.map((plugin) => (
            <div key={plugin.id} className="rule-row">
              <div className="rule-info">
                <div className="rule-title">
                  {plugin.name}
                  <span className="rule-tag">{plugin.target}</span>
                </div>
                <div className="rule-desc">{plugin.description}</div>
                <Metadata items={plugin.metadata} />
              </div>
              <span className="rule-tag">
                {plugin.isConnected ? 'Connected' : 'Not connected'}
              </span>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
