import { useCallback, useEffect, useState } from 'react';
import { UserX, UserCheck, HardDrive, Trash2 } from 'lucide-react';
import { gql } from '../api.js';
import Avatar from '../Avatar.jsx';

/**
 * Every account on the instance, and the things an administrator can do to one.
 *
 * These mutations all existed on the server with no way to reach them: an account could be
 * suspended, given a different storage quota, or deleted only by hand-writing GraphQL. The
 * approval queue above answers "should this person get in"; this answers everything after that.
 *
 * Creating accounts is deliberately absent — signup and invite codes are the only way in, so the
 * same username rules, rate limiting and approval apply to everybody.
 */
const USERS_QUERY = `{ users { username role state quotaBytes } }`;

const SET_STATE = `mutation SetState($user: String!, $state: String!) {
  setAccountState(username: $user, state: $state) { username state }
}`;

const SET_QUOTA = `mutation SetQuota($user: String!, $bytes: Int!) {
  setAccountQuota(username: $user, quotaBytes: $bytes) { username quotaBytes }
}`;

const DELETE_ACCOUNT = `mutation DeleteAccount($user: String!) { deleteAccount(username: $user) }`;

const MB = 1024 * 1024;

/** "Unlimited" is what a zero means, and an admin is never capped whatever the number says. */
function describeQuota(account) {
  if (account.role === 'admin') return 'unlimited (admin)';
  if (!account.quotaBytes) return 'unlimited';
  return `${Math.round(account.quotaBytes / MB)} MB`;
}

export default function AccountsPanel({ me, onUnauthorized }) {
  const [accounts, setAccounts] = useState([]);
  const [busy, setBusy] = useState(null);
  const [notice, setNotice] = useState('');

  const load = useCallback(async () => {
    try {
      const res = await gql(USERS_QUERY);
      const body = await res.json();
      setAccounts(body?.data?.users ?? []);
    } catch (err) {
      if (err.unauthorized) onUnauthorized?.();
    }
  }, [onUnauthorized]);

  useEffect(() => { load(); }, [load]);

  async function run(key, query, variables, message) {
    setBusy(key);
    setNotice('');
    try {
      const res = await gql(query, variables);
      const body = await res.json();
      if (body?.errors?.length) throw new Error(body.errors[0].message);
      if (message) setNotice(message);
      await load();
    } catch (err) {
      if (err.unauthorized) onUnauthorized?.();
      else setNotice(err.message);
    } finally {
      setBusy(null);
    }
  }

  function changeQuota(account) {
    const current = account.quotaBytes ? Math.round(account.quotaBytes / MB) : 0;
    const typed = window.prompt(
      `Storage quota for “${account.username}”, in MB.\n\n` +
      'This caps the spool — the staging area for files being passed between devices. ' +
      '0 means unlimited.',
      String(current)
    );
    if (typed === null) return;
    const mb = Number(typed);
    if (!Number.isFinite(mb) || mb < 0) {
      setNotice('That is not a number of megabytes.');
      return;
    }
    run(account.username, SET_QUOTA, { user: account.username, bytes: Math.round(mb * MB) },
      `${account.username} now has ${mb === 0 ? 'no' : `a ${mb} MB`} quota.`);
  }

  function remove(account) {
    // Typed, not clicked. This takes the account's devices, sessions, settings and credentials
    // with it, and there is no undo.
    const typed = window.prompt(
      `Delete “${account.username}”?\n\n` +
      'This removes their devices, saved session, synced settings and every sign-in. ' +
      'It cannot be undone.\n\n' +
      `Type ${account.username} to confirm.`
    );
    if (typed !== account.username) return;
    run(account.username, DELETE_ACCOUNT, { user: account.username },
      `${account.username} deleted.`);
  }

  return (
    <div className="card">
      <div className="card-header">
        <div>
          <div className="card-title">Accounts ({accounts.length})</div>
          <div className="card-subtitle">Everyone on this instance</div>
        </div>
      </div>

      {notice && <div className="empty-hint" style={{ padding: '10px 0' }}>{notice}</div>}

      <div className="rules-list">
        {accounts.map((account) => {
          const isSelf = account.username === me;
          const suspended = account.state === 'suspended';
          return (
            <div key={account.username} className="rule-row" style={{ display: 'flex', alignItems: 'center', gap: '12px' }}>
              <Avatar username={account.username} size={36} />
              <div className="rule-info" style={{ flex: 1 }}>
                <div className="rule-title">
                  {account.username}
                  {account.role === 'admin' && <span className="rule-tag">admin</span>}
                  {suspended && <span className="rule-tag">suspended</span>}
                  {account.state === 'pending' && <span className="rule-tag">pending</span>}
                </div>
                <div className="rule-desc">Quota: {describeQuota(account)}</div>
              </div>

              <div style={{ display: 'flex', gap: '8px' }}>
                <button
                  className="btn btn-secondary"
                  disabled={busy === account.username || account.role === 'admin'}
                  onClick={() => changeQuota(account)}
                  title={account.role === 'admin'
                    ? 'An administrator is never capped'
                    : 'Change the storage quota'}
                >
                  <HardDrive size={14} /> Quota
                </button>

                {/* You cannot suspend or delete yourself: locking the only administrator out of
                    their own server is not a mistake worth making reachable in one click. */}
                <button
                  className="btn btn-secondary"
                  disabled={busy === account.username || isSelf}
                  onClick={() => run(
                    account.username,
                    SET_STATE,
                    { user: account.username, state: suspended ? 'active' : 'suspended' },
                    suspended ? `${account.username} restored.` : `${account.username} suspended.`
                  )}
                  title={isSelf ? 'You cannot suspend yourself' : ''}
                >
                  {suspended ? <><UserCheck size={14} /> Restore</> : <><UserX size={14} /> Suspend</>}
                </button>

                <button
                  className="btn btn-secondary btn-danger"
                  disabled={busy === account.username || isSelf}
                  onClick={() => remove(account)}
                  title={isSelf ? 'You cannot delete yourself here' : 'Delete this account'}
                >
                  <Trash2 size={14} />
                </button>
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}
