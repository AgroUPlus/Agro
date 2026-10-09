/** Calls and helpers for first-run setup. The server side is `login::bootstrap` and `setup_status`. */

/** Whether this server is still waiting for its first administrator. A failed call says no. */
export async function setupStatus() {
  try {
    const res = await fetch('/api/v1/setup-status');
    if (!res.ok) return false;
    return (await res.json()).needsSetup === true;
  } catch {
    return false;
  }
}

/**
 * The setup token from a `#setup=` link, or an empty string. Read-only, so it is safe to call
 * from a render. The fragment is used because browsers never send it to a server, so the token
 * stays out of access logs and referrers.
 */
export function readSetupFragment() {
  const params = new URLSearchParams(window.location.hash.replace(/^#/, ''));
  return (params.get('setup') || '').trim().slice(0, 256);
}

/** Removes the token from the address bar and history. Idempotent. */
export function clearSetupFragment() {
  if (!new URLSearchParams(window.location.hash.replace(/^#/, '')).has('setup')) return;
  window.history.replaceState(null, '', window.location.pathname + window.location.search);
}

/** Creates the first administrator. Resolves to `{ username, passphrase }`; the passphrase is shown once. */
export async function bootstrapAdmin(setupToken, username) {
  const res = await fetch('/api/v1/bootstrap', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ setupToken: setupToken.trim(), username: username.trim() })
  });
  const body = await res.json().catch(() => ({}));
  if (!res.ok || !body.passphrase) {
    throw new Error(body.error || 'The administrator could not be created');
  }
  return { username: body.username, passphrase: body.passphrase };
}
