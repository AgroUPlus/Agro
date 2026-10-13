import { useCallback, useEffect, useState } from 'react';
import { gql } from '../api.js';
import { ConnectorRow, FeatureTile } from './admin/FeatureTiles.jsx';

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
    return <div className="empty-hint">{error || 'Loading…'}</div>;
  }

  const features = plugins.filter((p) => p.toggleable);
  const connectors = plugins.filter((p) => !p.toggleable);
  const on = features.filter((f) => f.isEnabled).length;

  return (
    <>
      {error && <div className="empty-hint backup-notice">{error}</div>}

      <div className="card">
        <div className="card-header">
          <div className="card-title">Features</div>
          <span className="feature-count">{on} of {features.length} on</span>
        </div>
        <p className="card-subtitle">A feature that is off is refused for everyone. Turning it back on loses nothing.</p>
        {groupByCategory(features).map(([category, group]) => (
          <section key={category} className="feature-group">
            <span className="stats-overline">{category}</span>
            <div className="feature-grid">
              {group.map((feature) => (
                <FeatureTile key={feature.id} feature={feature} busy={busy === feature.id} onToggle={toggle} />
              ))}
            </div>
          </section>
        ))}
      </div>

      <div className="card">
        <div className="card-title">Clients & connectors</div>
        <ul className="connector-rows">
          {connectors.map((plugin) => <ConnectorRow key={plugin.id} plugin={plugin} />)}
        </ul>
      </div>
    </>
  );
}
