import { useCallback, useEffect, useState } from 'react';
import { Link2, Trash2, ExternalLink, Copy, AlertTriangle } from 'lucide-react';
import { gql } from '../App.jsx';

/**
 * Every link this account has minted, with its hit count and a way to remove it.
 *
 * Links were previously write-only: a player could mint one and nothing could ever list, count or
 * revoke it. The counts here are aggregates and nothing more — the server records a number and a
 * timestamp, never who clicked (see migration 6 in `db.rs`).
 *
 * Unlike the older tabs this queries with GraphQL variables rather than interpolating the username
 * into the document, which is the pattern the rest should move to as they are touched.
 */
const LIST_QUERY = `query Links($user: String!) {
  links(userId: $user) {
    id kind target url label createdAt expiresAt clickCount lastClickedAt source
  }
}`;

const DELETE_MUTATION = `mutation DeleteLink($user: String!, $id: String!, $kind: String!) {
  deleteLink(userId: $user, id: $id, kind: $kind) { deleted navidromeCleanupRequired }
}`;

export default function LinksTab({ username, onUnauthorized }) {
  const [links, setLinks] = useState([]);
  const [notice, setNotice] = useState('');
  const [busy, setBusy] = useState(null);

  const load = useCallback(async () => {
    try {
      const res = await gql(LIST_QUERY, { user: username });
      const body = await res.json();
      setLinks(body?.data?.links ?? []);
    } catch (error) {
      if (error.unauthorized) onUnauthorized?.();
    }
  }, [username, onUnauthorized]);

  useEffect(() => {
    load();
    // Slower than the 2.5s node poll: a click count is not something anyone watches tick over, and
    // this list can be long.
    const timer = setInterval(load, 15000);
    return () => clearInterval(timer);
  }, [load]);

  async function remove(link) {
    setBusy(link.id);
    setNotice('');
    try {
      const res = await gql(DELETE_MUTATION, {
        user: username,
        id: link.id,
        kind: link.kind
      });
      const body = await res.json();
      if (body?.errors?.length) {
        setNotice(body.errors[0].message);
      } else {
        if (body?.data?.deleteLink?.navidromeCleanupRequired) {
          setNotice(
            'Removed from Agro. The underlying share still exists on Navidrome — Agro never ' +
              'holds your Navidrome password, so it cannot revoke it for you. Delete it from ' +
              'Navidrome to stop the audio being served.'
          );
        }
        setLinks(current => current.filter(item => item.id !== link.id));
      }
    } catch (error) {
      if (error.unauthorized) onUnauthorized?.();
      else setNotice('Could not reach the server.');
    } finally {
      setBusy(null);
    }
  }

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: '16px' }}>
      {notice && (
        <div className="card" style={{ display: 'flex', gap: '10px', alignItems: 'flex-start' }}>
          <AlertTriangle size={16} style={{ flexShrink: 0, marginTop: '2px' }} />
          <div style={{ fontSize: '13px' }}>{notice}</div>
        </div>
      )}

      <div className="card">
        <div className="card-header">
          <div className="card-title">Links ({links.length})</div>
        </div>

        {links.length === 0 ? (
          <div className="empty-hint">
            No links yet. Share a track from <strong>wander</strong> or <strong>wanda</strong> and
            it will appear here.
          </div>
        ) : (
          <div style={{ overflowX: 'auto' }}>
            <table className="data-table">
              <thead>
                <tr>
                  <th>Link</th>
                  <th>Goes to</th>
                  <th>Clicks</th>
                  <th>Created</th>
                  <th>Expires</th>
                  <th />
                </tr>
              </thead>
              <tbody>
                {links.map(link => (
                  <tr key={`${link.kind}:${link.id}`}>
                    <td>
                      <div style={{ display: 'flex', alignItems: 'center', gap: '6px' }}>
                        <Link2 size={13} />
                        <span>{link.label || link.id}</span>
                      </div>
                      <div className="row-sub">
                        {link.kind === 'SHORT' ? 'Forwarding link' : 'Hosted share'}
                        {link.source ? ` · ${link.source}` : ''}
                      </div>
                    </td>
                    <td className="truncate" title={link.target}>
                      {link.target}
                    </td>
                    <td>
                      {link.clickCount}
                      {link.lastClickedAt ? (
                        <div className="row-sub">last {formatWhen(link.lastClickedAt)}</div>
                      ) : null}
                    </td>
                    <td>{formatWhen(link.createdAt)}</td>
                    <td>{formatExpiry(link.expiresAt)}</td>
                    <td>
                      <div style={{ display: 'flex', gap: '6px', justifyContent: 'flex-end' }}>
                        <button
                          className="icon-btn"
                          title="Copy link"
                          onClick={() => navigator.clipboard?.writeText(link.url)}
                        >
                          <Copy size={14} />
                        </button>
                        <a
                          className="icon-btn"
                          title="Open link"
                          href={link.url}
                          target="_blank"
                          rel="noreferrer noopener"
                        >
                          <ExternalLink size={14} />
                        </a>
                        <button
                          className="icon-btn danger"
                          title="Delete link"
                          disabled={busy === link.id}
                          onClick={() => remove(link)}
                        >
                          <Trash2 size={14} />
                        </button>
                      </div>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </div>
    </div>
  );
}

function formatWhen(unixSeconds) {
  if (!unixSeconds) return '—';
  return new Date(unixSeconds * 1000).toLocaleString();
}

function formatExpiry(unixSeconds) {
  if (!unixSeconds) return 'Never';
  return unixSeconds * 1000 < Date.now() ? 'Expired' : formatWhen(unixSeconds);
}
