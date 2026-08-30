/**
 * Centralized API & Authentication Client for Agro Dashboard.
 */

const TOKEN_KEY = 'agro.token';

export function getToken() {
  return localStorage.getItem(TOKEN_KEY) || '';
}

export function setToken(value) {
  if (value) localStorage.setItem(TOKEN_KEY, value.trim());
  else localStorage.removeItem(TOKEN_KEY);
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
      ...(totpCode ? { totp_code: totpCode.trim() } : {}),
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

  const error = params.get('ssoError');
  const token = params.get('token');
  if (!error && !token && !params.has('linked')) return null;

  window.history.replaceState(null, '', window.location.pathname + window.location.search);

  if (error) return { error };
  if (params.has('linked') && !token) return { linked: true };

  setToken(token);
  return {
    token,
    username: params.get('username') || '',
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

export async function gql(query, variables = {}) {
  const token = getToken();
  const res = await fetch('/graphql', {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      ...(token ? { Authorization: `Bearer ${token}` } : {}),
    },
    body: JSON.stringify({ query, variables }),
  });

  if (res.status === 401) {
    const error = new Error('Unauthorized');
    error.unauthorized = true;
    throw error;
  }

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

export const FALLBACK_RULES = [
  {
    id: 'auto-handoff',
    name: 'Spotify-Style Playback Hand-off',
    description: 'Relays active track URI, metadata, and millisecond timestamp across Wander (Desktop) and Wanda (Android) so resuming on any device is a 1-tap/key prompt.',
    target: 'Wanda ↔ Wander',
    isEnabled: true
  },
  {
    id: 'settings-sync',
    name: 'Cross-Device Settings Synchronizer',
    description: 'Automatically synchronizes shared Subsonic credentials, LRCLIB lyrics resolvers, audio quality, and plugin keys between desktop and mobile.',
    target: 'All Clients',
    isEnabled: true
  },
  {
    id: 'wifi-precache',
    name: 'Proactive Wi-Fi Smart Pre-Caching',
    description: 'Instructs Wanda to automatically pre-download the top 15 next queued tracks over unmetered Wi-Fi connections.',
    target: 'Wanda Mobile',
    isEnabled: true
  },
  {
    id: 'lrclib-lyrics-hub',
    name: 'Central LRCLIB Synced Lyrics Hub',
    description: 'Background LRCLIB resolver that fetches and caches synchronized LRC lyrics in SQLite for all connected devices with zero duplicate queries.',
    target: 'All Clients',
    isEnabled: true
  },
  {
    id: 'listen-along',
    name: 'Listen Along',
    description: 'Pushes a host\'s track and position to the friends following them, so a session can be joined rather than merely watched. Gated on the host\'s "show now playing" switch.',
    target: 'Multi-device',
    isEnabled: true
  }
];
