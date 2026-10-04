/**
 * Opening what an account's own devices seal — for now, what is playing.
 *
 * Wanda seals its playback handoff under a subkey of the account's vault key, which this server
 * never holds: the dashboard only ever saw "Private Session". The vault key travels sealed under
 * the passphrase (Argon2id, then AES-256-GCM), so a dashboard signed in with that passphrase can
 * unwrap it here, exactly as Wanda's `AgroVault` does, and derive the one subkey it needs.
 *
 * Only the presence subkey is kept, and only in `sessionStorage`: it opens "what is playing" and
 * nothing else — not synced settings, not backups — and it is gone when the tab closes. The vault
 * key and the passphrase are never stored.
 */
import { argon2id } from 'hash-wasm';

const PRESENCE_KEY = 'agro.presenceKey';

// RFC 9106's second recommendation, as `AgroVault` derives it. Changing any of these changes the key.
const ARGON2 = { parallelism: 4, iterations: 3, memorySize: 65536, hashLength: 32 };
const NONCE_BYTES = 12;
const INFO_PRESENCE = 'agro/v1/presence';

const utf8 = (text) => new TextEncoder().encode(text);
const fromBase64 = (text) => Uint8Array.from(atob(text), (c) => c.charCodeAt(0));
const toBase64 = (bytes) => btoa(String.fromCharCode(...bytes));
const fromHex = (hex) => Uint8Array.from(hex.trim().match(/../g) ?? [], (pair) => parseInt(pair, 16));

/** `nonce || ciphertext || tag`, as every sealed value from a device is laid out. */
async function openSealed(sealed, rawKey) {
  const key = await crypto.subtle.importKey('raw', rawKey, 'AES-GCM', false, ['decrypt']);
  const plain = await crypto.subtle.decrypt(
    { name: 'AES-GCM', iv: sealed.slice(0, NONCE_BYTES) },
    key,
    sealed.slice(NONCE_BYTES)
  );
  return new Uint8Array(plain);
}

/**
 * Unwraps the vault key with [passphrase] and keeps its presence subkey for this tab.
 *
 * @throws if the passphrase does not open the envelope.
 */
export async function unlockPresence(passphrase, vaultSalt, vaultKeyWrapped) {
  const wrappingKey = await argon2id({
    ...ARGON2,
    // Trimmed, as `AgroVault.deriveWrappingKey` trims: the same passphrase must give the same key.
    password: passphrase.trim(),
    salt: fromHex(vaultSalt),
    outputType: 'binary',
  });
  const vaultKey = await openSealed(fromBase64(vaultKeyWrapped), wrappingKey);
  // HKDF-SHA256 with an empty salt, as `AgroVault.deriveSubkey`.
  const root = await crypto.subtle.importKey('raw', vaultKey, 'HKDF', false, ['deriveBits']);
  const bits = await crypto.subtle.deriveBits(
    { name: 'HKDF', hash: 'SHA-256', salt: new Uint8Array(0), info: utf8(INFO_PRESENCE) },
    root,
    256
  );
  vaultKey.fill(0);
  wrappingKey.fill(0);
  sessionStorage.setItem(PRESENCE_KEY, toBase64(new Uint8Array(bits)));
}

export function forgetPresence() {
  sessionStorage.removeItem(PRESENCE_KEY);
}

export function canOpenPresence() {
  return sessionStorage.getItem(PRESENCE_KEY) !== null;
}

/**
 * What a sealed handoff says is playing, or null when this tab holds no key or the key does not
 * open it — sealed by another account's vault, or before the vault key changed.
 */
export async function openPresence(encryptedPayload) {
  const stored = sessionStorage.getItem(PRESENCE_KEY);
  if (!stored || !encryptedPayload) return null;
  try {
    const plain = await openSealed(fromBase64(encryptedPayload), fromBase64(stored));
    return JSON.parse(new TextDecoder().decode(plain));
  } catch {
    return null;
  }
}
