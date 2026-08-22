import { useCallback, useEffect, useState } from 'react';
import { UserCheck, UserX, Ticket, Copy, Ban, Trash2 } from 'lucide-react';
import { gql } from '../App.jsx';
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
    <div style={{ display: 'flex', flexDirection: 'column', gap: '16px' }}>
      {notice && <div className="card" style={{ color: 'var(--danger, #f66)' }}>{notice}</div>}

      <div className="card">
        <div className="card-header">
          <div className="card-title">Waiting for approval ({pending.length})</div>
        </div>
        {pending.length === 0 ? (
          <p style={{ opacity: 0.6 }}>Nobody is waiting. New signups appear here.</p>
        ) : (
          pending.map((account) => (
            <div
              key={account.username}
              style={{
                display: 'flex',
                alignItems: 'center',
                justifyContent: 'space-between',
                gap: '12px',
                padding: '8px 0'
              }}
            >
              <div style={{ display: 'flex', alignItems: 'center', gap: '12px' }}>
                <Avatar username={account.username} size={36} />
                <div>
                  <strong>{account.username}</strong>
                  <div style={{ opacity: 0.6, fontSize: '12px' }}>
                    signed up {new Date(account.createdAt).toLocaleString()}
                  </div>
                </div>
              </div>
              <div style={{ display: 'flex', gap: '8px' }}>
                <button
                  className="btn"
                  disabled={busy === account.username}
                  onClick={() => decide(account.username, 'active')}
                >
                  <UserCheck size={14} /> Let in
                </button>
                {/* Suspended rather than deleted: the account keeps its name, so approving
                    somebody by mistake is undoable and the name cannot be re-registered. */}
                <button
                  className="btn"
                  disabled={busy === account.username}
                  onClick={() => decide(account.username, 'suspended')}
                >
                  <UserX size={14} /> Refuse
                </button>
              </div>
            </div>
          ))
        )}
      </div>

      <div className="card">
        <div className="card-header">
          <div className="card-title">Invite codes</div>
          <div style={{ display: 'flex', gap: '8px' }}>
            <button className="btn" disabled={busy === 'new-invite'} onClick={() => mintInvite(1, null)}>
              <Ticket size={14} /> One-time code
            </button>
            <button className="btn" disabled={busy === 'new-invite'} onClick={() => mintInvite(25, 168)}>
              <Ticket size={14} /> 25 uses, 7 days
            </button>
          </div>
        </div>
        <p style={{ opacity: 0.6, fontSize: '12px' }}>
          Invite codes allow new users to bypass the approval queue and join immediately. Newly
          generated codes are automatically copied to your clipboard.
        </p>
        {invites.map((invite) => (
          <div
            key={invite.code}
            style={{
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'space-between',
              gap: '12px',
              padding: '8px 0',
              opacity: invite.revoked || invite.usedCount >= invite.maxUses ? 0.4 : 1
            }}
          >
            <div>
              <code>{invite.code}</code>
              <div style={{ opacity: 0.6, fontSize: '12px' }}>
                {invite.usedCount}/{invite.maxUses} used
                {invite.expiresAt && ` · expires ${new Date(invite.expiresAt).toLocaleString()}`}
                {invite.revoked && ' · revoked'}
              </div>
            </div>
            <div style={{ display: 'flex', gap: '8px' }}>
              <button className="btn" onClick={() => copy(invite.code)}>
                <Copy size={14} /> {copied === invite.code ? 'Copied' : 'Copy'}
              </button>
              {!invite.revoked && (
                <button
                  className="btn"
                  disabled={busy === invite.code}
                  onClick={() => run(invite.code, REVOKE_INVITE, { code: invite.code })}
                >
                  <Ban size={14} /> Revoke
                </button>
              )}
              {/* Revoking stops a code working and keeps the record; deleting is the tidying-up
                  once it is spent or dead. Offered only then, so a live code cannot be removed
                  from the list while it still lets people in. */}
              {(invite.revoked || invite.usedCount >= invite.maxUses) && (
                <button
                  className="btn"
                  disabled={busy === invite.code}
                  onClick={() => run(invite.code, DELETE_INVITE, { code: invite.code })}
                  title="Remove this code from the list"
                >
                  <Trash2 size={14} /> Delete
                </button>
              )}
            </div>
          </div>
        ))}
      </div>

      <AccountsPanel me={me} onUnauthorized={onUnauthorized} />
    </div>
  );
}
