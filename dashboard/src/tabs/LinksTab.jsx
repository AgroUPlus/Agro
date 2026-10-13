import { useCallback, useEffect, useState } from 'react';
import { Link2, Share2, Trash2, ExternalLink, Copy, AlertTriangle } from 'lucide-react';
import { gql } from '../api.js';
import Cookie from '../expressive/Cookie.jsx';

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
    id kind target url label expiresAt clickCount lastClickedAt
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
            'Removed from Agro. The underlying share still exists on your music server — Agro never ' +
              'holds its password, so it cannot revoke it for you. Delete it from ' +
              'the server to stop the audio being served.'
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
    <>
      {notice && <div className="empty-hint backup-notice"><AlertTriangle size={16} />{notice}</div>}

      {links.length === 0 ? (
        <div className="empty-hint">No links yet. Share a track from Wanda or Wander.</div>
      ) : (
        <div className="card">
          <ul className="link-rows">
            {links.map(link => {
              const forwarding = link.kind === 'SHORT';
              return (
                <li key={`${link.kind}:${link.id}`} className="link-row">
                  <Cookie shape={forwarding ? 'clover' : 'cookie'} size={48} className="link-shape">
                    {forwarding ? <Link2 size={20} /> : <Share2 size={20} />}
                  </Cookie>
                  <span className="link-text">
                    <span className="link-label">{link.label || link.id}</span>
                    <span className="link-target" title={link.target}>{link.target}</span>
                  </span>
                  <span className={`link-expiry ${link.expiresAt && link.expiresAt * 1000 < Date.now() ? 'is-expired' : ''}`}>
                    {formatExpiry(link.expiresAt)}
                  </span>
                  <span className="link-clicks" title={link.lastClickedAt ? `Last opened ${formatWhen(link.lastClickedAt)}` : 'Never opened'}>
                    <b>{link.clickCount}</b>clicks
                  </span>
                  <span className="link-actions">
                    <button className="icon-btn" aria-label="Copy link" onClick={() => navigator.clipboard?.writeText(link.url)}>
                      <Copy size={15} />
                    </button>
                    <a className="icon-btn" aria-label="Open link" href={link.url} target="_blank" rel="noreferrer noopener">
                      <ExternalLink size={15} />
                    </a>
                    <button className="icon-btn danger" aria-label="Delete link" disabled={busy === link.id} onClick={() => remove(link)}>
                      <Trash2 size={15} />
                    </button>
                  </span>
                </li>
              );
            })}
          </ul>
        </div>
      )}
    </>
  );
}

function formatWhen(unixSeconds) {
  if (!unixSeconds) return '—';
  return new Date(unixSeconds * 1000).toLocaleString();
}

function formatExpiry(unixSeconds) {
  if (!unixSeconds) return 'Never expires';
  if (unixSeconds * 1000 < Date.now()) return 'Expired';
  return `Until ${new Date(unixSeconds * 1000).toLocaleDateString()}`;
}
