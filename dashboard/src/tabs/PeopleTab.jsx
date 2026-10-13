import { useCallback, useEffect, useState } from 'react';
import { UserCheck, UserX, Ticket, Copy, Check, Ban, Trash2, Plus, Hourglass } from 'lucide-react';
import { gql } from '../api.js';
import AccountsPanel from './AccountsPanel.jsx';
import Avatar from '../Avatar.jsx';

/**
 * The approval queue and the invite codes — the two things an open instance cannot run without.
 *
 * `POST /api/v1/signup` creates accounts in the `pending` state and nothing else can let them in,
 * so without this screen a public server accepts registrations that no one is able to approve.
 */
const PENDING_QUERY = `{ pendingAccounts { username createdAt } }`;

const INVITES_QUERY = `{ invites { code createdBy createdAt expiresAt maxUses usedCount revoked } }`;

const APPROVE = `mutation Approve($user: String!, $state: String!) {
  setAccountState(username: $user, state: $state) { username state }
}`;

const CREATE_INVITE = `mutation CreateInvite($maxUses: Int, $ttlHours: Int) {
  createInvite(maxUses: $maxUses, ttlHours: $ttlHours) { code }
}`;

const REVOKE_INVITE = `mutation RevokeInvite($code: String!) { revokeInvite(code: $code) }`;

const DELETE_INVITE = `mutation DeleteInvite($code: String!) { deleteInvite(code: $code) }`;

export default function PeopleTab({ me, onUnauthorized }) {
  const [pending, setPending] = useState([]);
  const [invites, setInvites] = useState([]);
  const [notice, setNotice] = useState('');
  const [busy, setBusy] = useState(null);
  const [copied, setCopied] = useState('');

  const load = useCallback(async () => {
    try {
      const [pendingRes, invitesRes] = await Promise.all([
        gql(PENDING_QUERY),
        gql(INVITES_QUERY)
      ]);
      const pendingBody = await pendingRes.json();
      const invitesBody = await invitesRes.json();
      setPending(pendingBody?.data?.pendingAccounts ?? []);
      setInvites(invitesBody?.data?.invites ?? []);
    } catch (error) {
      if (error.unauthorized) onUnauthorized?.();
    }
  }, [onUnauthorized]);

  useEffect(() => {
    load();
    // Someone signing up is not an event anyone watches tick over.
    const timer = setInterval(load, 20000);
    return () => clearInterval(timer);
  }, [load]);

  async function run(key, query, variables) {
    setBusy(key);
    setNotice('');
    try {
      const body = await (await gql(query, variables)).json();
      if (body?.errors?.length) setNotice(body.errors[0].message);
      await load();
      return body?.data;
    } catch (error) {
      if (error.unauthorized) onUnauthorized?.();
      return null;
    } finally {
      setBusy(null);
    }
  }

  async function decide(username, state) {
    await run(username, APPROVE, { user: username, state });
  }

  async function mintInvite(maxUses, ttlHours) {
    const data = await run('new-invite', CREATE_INVITE, { maxUses, ttlHours });
    if (data?.createInvite?.code) copy(data.createInvite.code);
  }

  function copy(code) {
    navigator.clipboard?.writeText(code);
    setCopied(code);
    setTimeout(() => setCopied(''), 2000);
  }

  return (
    <>
      {notice && <div className="empty-hint backup-notice">{notice}</div>}

      {/* Shown only while someone is waiting: an empty queue needs no card saying so. */}
      {pending.length > 0 && (
        <div className="card queue-card">
          <div className="card-header">
            <div className="card-title"><Hourglass size={18} />Waiting to join</div>
            <span className="feature-count">{pending.length}</span>
          </div>
          <ul className="key-rows">
            {pending.map((account) => (
              <li key={account.username} className="key-row">
                <Avatar username={account.username} size={44} />
                <span className="key-text">
                  <span className="key-label">{account.username}</span>
                  <span className="key-sub">Signed up {new Date(account.createdAt).toLocaleString()}</span>
                </span>
                <button
                  className="btn btn-primary"
                  disabled={busy === account.username}
                  onClick={() => decide(account.username, 'active')}
                >
                  <UserCheck size={15} /> Let in
                </button>
                {/* Suspended rather than deleted: the account keeps its name, so approving
                    somebody by mistake is undoable and the name cannot be re-registered. */}
                <button
                  className="icon-btn danger"
                  disabled={busy === account.username}
                  onClick={() => decide(account.username, 'suspended')}
                  aria-label="Refuse"
                  title="Refuse"
                >
                  <UserX size={16} />
                </button>
              </li>
            ))}
          </ul>
        </div>
      )}

      <div className="card">
        <div className="card-header">
          <div className="card-title"><Ticket size={18} />Invites</div>
          <div className="invite-make">
            <button className="btn btn-secondary" disabled={busy === 'new-invite'} onClick={() => mintInvite(1, null)}>
              <Plus size={15} /> Single use
            </button>
            <button className="btn btn-secondary" disabled={busy === 'new-invite'} onClick={() => mintInvite(25, 168)}>
              <Plus size={15} /> 25 people, 7 days
            </button>
          </div>
        </div>
        <p className="card-subtitle">A code skips the approval queue. New codes are copied for you.</p>
        {invites.length > 0 && (
          <div className="ticket-grid">
            {invites.map((invite) => {
              const spent = invite.revoked || invite.usedCount >= invite.maxUses;
              return (
                <div key={invite.code} className={`ticket ${spent ? 'is-spent' : ''}`}>
                  <code className="ticket-code">{invite.code}</code>
                  <span className="ticket-meter"><span style={{ width: `${(invite.usedCount / invite.maxUses) * 100}%` }} /></span>
                  <span className="ticket-sub">
                    {invite.usedCount}/{invite.maxUses} used
                    {invite.revoked ? ' · revoked' : invite.expiresAt ? ` · until ${new Date(invite.expiresAt).toLocaleDateString()}` : ''}
                  </span>
                  <span className="ticket-actions">
                    <button className="icon-btn" onClick={() => copy(invite.code)} aria-label="Copy code">
                      {copied === invite.code ? <Check size={15} /> : <Copy size={15} />}
                    </button>
                    {!invite.revoked && (
                      <button
                        className="icon-btn"
                        disabled={busy === invite.code}
                        onClick={() => run(invite.code, REVOKE_INVITE, { code: invite.code })}
                        aria-label="Revoke"
                        title="Revoke"
                      >
                        <Ban size={15} />
                      </button>
                    )}
                    {/* Revoking stops a code working and keeps the record; deleting is the tidying-up
                        once it is spent or dead. Offered only then, so a live code cannot be removed
                        from the list while it still lets people in. */}
                    {spent && (
                      <button
                        className="icon-btn danger"
                        disabled={busy === invite.code}
                        onClick={() => run(invite.code, DELETE_INVITE, { code: invite.code })}
                        aria-label="Remove"
                        title="Remove this code from the list"
                      >
                        <Trash2 size={15} />
                      </button>
                    )}
                  </span>
                </div>
              );
            })}
          </div>
        )}
      </div>

      <AccountsPanel me={me} onUnauthorized={onUnauthorized} />
    </>
  );
}
