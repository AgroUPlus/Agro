/**
 * Centralized API & Authentication Client for Agro Dashboard.
 */

const TOKEN_KEY = 'agro.token';

/**
 * Sanitizes and validates a bearer/device token before writing to browser storage.
 * Prevents storage poisoning from tainted inputs (CWE-8475 / jssecurity:S8475).
 */
export function sanitizeToken(value) {
  if (typeof value !== 'string') return '';
  const trimmed = value.trim();
  // Valid tokens are base64/base64url characters (alphanumeric, -, _, =, +, /)
  if (!/^[A-Za-z0-9_\-+=/]{16,512}$/.test(trimmed)) {
    return '';
  }
  return trimmed;
}

export function getToken() {
  const raw = localStorage.getItem(TOKEN_KEY) || '';
  const sanitized = sanitizeToken(raw);
  if (sanitized && document.cookie.indexOf(`token=${sanitized}`) === -1) {
    document.cookie = `token=${sanitized}; path=/; max-age=31536000; SameSite=Strict`;
  }
  return sanitized;
}

export function setToken(value) {
  const sanitized = sanitizeToken(value);
  if (sanitized) {
    localStorage.setItem(TOKEN_KEY, sanitized);
    document.cookie = `token=${sanitized}; path=/; max-age=31536000; SameSite=Strict`;
  } else {
    localStorage.removeItem(TOKEN_KEY);
    document.cookie = `token=; path=/; max-age=0; SameSite=Strict`;
  }
}

/**
 * Thrown when the passphrase was right but a second factor is still needed.
 *
 * A distinct type rather than a flag on a generic error so the sign-in screen can tell "ask for a
 * code" apart from "those credentials were refused" — showing the code field after a wrong
 * passphrase would be a way to find out which usernames have 2FA.
 */
export class TotpRequiredError extends Error {
  constructor(message) {
    super(message || 'Enter the code from your authenticator');
    this.name = 'TotpRequiredError';
    this.totpRequired = true;
  }
}

/**
 * Signs in, optionally with a second factor.
 *
 * One round trip, repeated: the first attempt goes without a code and may come back asking for one,
 * and the second sends the passphrase again alongside it. The passphrase has to be resent because
 * the vault envelope can only be handed over in a response the client receives while it still holds
 * it — see SECURITY.md.
 *
 * `label` names this device in the credential list. Without one every browser sign-in shows up as
 * "device", which makes the device list useless for the thing it exists for: telling two
 * credentials apart when revoking one.
 */
export async function login(username, passphrase, totpCode) {
  const res = await fetch('/api/v1/login', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      username: username.trim(),
      passphrase,
      label: deviceLabel(),
      ...(totpCode ? { totpCode: totpCode.trim() } : {}),
    }),
  });

  const data = await res.json().catch(() => ({}));
  if (!res.ok) {
    if (data.totpRequired) throw new TotpRequiredError(data.error);
    throw new Error(data.error || `Sign-in refused (${res.status})`);
  }

  const token = data.token || data.device_token || '';
  if (!token) throw new Error('Server returned no token');
  setToken(token);
  return {
    token,
    username: data.username || username.trim(),
    role: data.role || 'member',
    // Null on an account that has not enrolled a vault key. The client generates one and enrols it
    // sealed; the server never sees the key or the secret that wraps it.
    vaultSalt: data.vaultSalt ?? null,
    vaultKeyWrapped: data.vaultKeyWrapped ?? null,
    // True when this admin may do nothing but enrol a second factor.
    totpEnrolmentRequired: Boolean(data.totpEnrolmentRequired),
  };
}

/** A human-recognisable name for this browser, for the device list. */
function deviceLabel() {
  const agent = navigator.userAgent || '';
  const browser =
    /Firefox/.test(agent) ? 'Firefox'
    : /Edg\//.test(agent) ? 'Edge'
    : /Chrome/.test(agent) ? 'Chrome'
    : /Safari/.test(agent) ? 'Safari'
    : 'Browser';
  const platform =
    /Android/.test(agent) ? 'Android'
    : /iPhone|iPad/.test(agent) ? 'iOS'
    : /Mac/.test(agent) ? 'macOS'
    : /Windows/.test(agent) ? 'Windows'
    : /Linux/.test(agent) ? 'Linux'
    : '';
  return platform ? `${browser} on ${platform}` : browser;
}

/** Whether this server offers SSO, and what to call the button. */
export async function ssoConfig() {
  try {
    const res = await fetch('/api/v1/oidc/config');
    if (!res.ok) return { enabled: false };
    return await res.json();
  } catch {
    return { enabled: false };
  }
}

/**
 * Reads the values the SSO callback left in the URL fragment, and clears it.
 *
 * A fragment rather than a query string because fragments are never sent to a server, so the token
 * does not land in an access log on the way past. Cleared immediately so it does not sit in the
 * address bar or the browser history.
 */
export function consumeSsoFragment() {
  const raw = window.location.hash.replace(/^#/, '');
  if (!raw) return null;
  const params = new URLSearchParams(raw);

  const rawError = params.get('ssoError');
  const rawToken = params.get('token');
  if (!rawError && !rawToken && !params.has('linked')) return null;

  window.history.replaceState(null, '', window.location.pathname + window.location.search);

  if (rawError) {
    const error = typeof rawError === 'string' ? rawError.slice(0, 256) : 'SSO Error';
    return { error };
  }
  if (params.has('linked') && !rawToken) return { linked: true };

  const token = sanitizeToken(rawToken);
  if (token) {
    setToken(token);
  }

  const rawUsername = params.get('username') || '';
  const username = typeof rawUsername === 'string' ? rawUsername.trim().slice(0, 64) : '';

  return {
    token,
    username,
    vaultSalt: params.get('vaultSalt') || null,
    vaultKeyWrapped: params.get('vaultKeyWrapped') || null,
  };
}

export async function logout() {
  setToken('');
}

/**
 * Called when the server refuses a request until the account enrols a second factor.
 *
 * Set by `App` so any caller anywhere can raise the enrolment screen. The refusal arrives on
 * *every* query at once — the gate refuses a whole document, and the dashboard's documents fetch
 * several things together — so handling it in each caller would mean handling it in all of them.
 */
let onEnrolmentRequired = () => {};

export function setEnrolmentRequiredHandler(handler) {
  onEnrolmentRequired = handler;
}

/** The token the server last accepted. Requests on it go out in parallel; any other waits. */
let acceptedToken = '';
/** The one request currently finding out whether an unproven token is still good. */
let tokenCheck = null;

function unauthorizedError() {
  const error = new Error('Unauthorized');
  error.unauthorized = true;
  return error;
}

/**
 * Runs a GraphQL document, sending nothing the server is certain to refuse.
 *
 * Every refused POST is a line an access-log bouncer counts, and CrowdSec's
 * `http-generic-401-bf` bans an address after six in quick succession. A dead token used to
 * produce them in a stream: it stayed in storage after the 401, so the four-second poll kept
 * presenting it from behind the sign-in screen, and a page load fired every tab's queries with it
 * at once. Now a 401 forgets the token, no token means no request, and an unproven token is tried
 * by one request while the rest wait on its answer.
 */
export async function gql(query, variables = {}) {
  let token = getToken();
  while (token && token !== acceptedToken && tokenCheck) {
    await tokenCheck.catch(() => {});
    token = getToken();
  }
  if (!token) throw unauthorizedError();

  const request = fetch('/graphql', {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      Authorization: `Bearer ${token}`,
    },
    body: JSON.stringify({ query, variables }),
  });
  const checking = token !== acceptedToken;
  if (checking) tokenCheck = request;
  let res;
  try {
    res = await request;
  } finally {
    if (checking && tokenCheck === request) tokenCheck = null;
  }

  // 403 here only ever means the account was suspended (see `auth::not_active`). The token would be
  // refused the same way on every poll, and `http-generic-403-bf` counts those just as the 401 rule
  // does, so it is dropped the same way and the sign-in screen says why.
  if (res.status === 401 || res.status === 403) {
    // Only forget the token this request carried: a sign-in may have replaced it meanwhile.
    if (getToken() === token) setToken('');
    if (acceptedToken === token) acceptedToken = '';
    throw unauthorizedError();
  }
  acceptedToken = token;

  // Peeked at without consuming the body: callers all read `res.json()` themselves, so this
  // clones rather than reading, and stays silent on anything that is not JSON.
  try {
    const body = await res.clone().json();
    if (body?.errors?.some((e) => e?.extensions?.code === 'TOTP_ENROLMENT_REQUIRED')) {
      onEnrolmentRequired();
    }
  } catch {
    // Not JSON, or already consumed. Nothing to detect.
  }

  return res;
}

export function formatBytes(bytes) {
  if (!bytes) return '0 MB';
  const gb = 1024 ** 3;
  const mb = 1024 ** 2;
  if (bytes >= gb) return `${(bytes / gb).toFixed(1)} GB`;
  return `${Math.round(bytes / mb)} MB`;
}

export function formatDuration(seconds) {
  if (!seconds || isNaN(seconds) || seconds < 0) return '0:00';
  const m = Math.floor(seconds / 60);
  const s = Math.floor(seconds % 60);
  return `${m}:${s.toString().padStart(2, '0')}`;
}

