//! Password hashing and bearer-token minting.
//!
//! Two different jobs that deliberately use two different primitives.
//!
//! **Passphrases are hashed with Argon2id.** A human-chosen passphrase has little entropy, so the
//! only thing standing between a stolen database and the account is how expensive each guess is.
//! Argon2 is slow and memory-hard on purpose.
//!
//! **Tokens are hashed with SHA-256.** A token is 256 bits from the OS CSPRNG — there is no
//! dictionary to run against it, and no amount of hashing makes guessing 2^256 easier. What matters
//! is that the database stores something that cannot be *replayed*, and a fast digest does that.
//! Using Argon2 here would be worse than useless: this runs on every single authenticated request,
//! so a memory-hard hash in that path is a denial-of-service the server performs on itself.
//!
//! Before this module the account passphrase *was* the bearer token, stored in the clear in
//! `users.api_key`, and app-password tokens were stored in the clear too. A single read of the
//! database — a backup, a stray log line — disclosed every credential on the server, and there was
//! nothing to rotate to, because the passphrase and the token were the same string.

use argon2::password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use rand::RngCore;
use sha2::{Digest, Sha256};

/// How much of a token is stored in the clear, so a presented token can be looked up by index
/// rather than by hashing every row. Eight characters of base64url is 48 bits — far too little to
/// guess the rest from, and enough that collisions are not a practical concern.
pub const TOKEN_PREFIX_LEN: usize = 8;

/// Hashes a passphrase for storage. The salt is generated per call.
pub fn hash_passphrase(passphrase: &str) -> Result<String, String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(passphrase.trim().as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|e| format!("could not hash passphrase: {e}"))
}

/// Verifies a passphrase against a stored Argon2 hash.
///
/// An unparseable or empty stored hash verifies as `false` rather than erroring: an account whose
/// hash never got written must be impossible to log into, not a way to bypass the check.
pub fn verify_passphrase(passphrase: &str, stored_hash: &str) -> bool {
    if stored_hash.is_empty() {
        return false;
    }
    let Ok(parsed) = PasswordHash::new(stored_hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(passphrase.trim().as_bytes(), &parsed)
        .is_ok()
}

/// A freshly minted bearer token, and the two values that go in the database.
///
/// The `secret` is returned to the caller exactly once, at creation, and is never recoverable
/// afterwards — the server keeps only [`Self::hash`].
pub struct MintedToken {
    pub secret: String,
    pub prefix: String,
    pub hash: String,
}

/// Mints a new device token from the OS CSPRNG.
pub fn mint_token() -> MintedToken {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let secret = base64url(&bytes);
    let prefix = secret.chars().take(TOKEN_PREFIX_LEN).collect::<String>();
    let hash = hash_token(&secret);
    MintedToken { secret, prefix, hash }
}

/// The stored form of a token. Deterministic, so a presented token can be matched against it.
pub fn hash_token(token: &str) -> String {
    let digest = Sha256::digest(token.trim().as_bytes());
    hex::encode(digest)
}

/// The indexed lookup key for a presented token.
pub fn token_prefix(token: &str) -> String {
    token.trim().chars().take(TOKEN_PREFIX_LEN).collect()
}

/// Constant-time comparison, so a token cannot be recovered by timing the failure.
///
/// Both sides here are hex digests of a fixed length, but the comparison is still done this way:
/// the rule is cheap to keep and expensive to remember to add back later.
pub fn secure_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// URL-safe base64 without padding, so a token survives a query string and a QR payload unescaped.
fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        let indices = [(n >> 18) & 63, (n >> 12) & 63, (n >> 6) & 63, n & 63];
        // A 1-byte tail encodes to 2 characters and a 2-byte tail to 3; the rest is padding we
        // do not emit.
        let keep = chunk.len() + 1;
        for &i in indices.iter().take(keep) {
            out.push(ALPHABET[i as usize] as char);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passphrase_round_trips() {
        let hash = hash_passphrase("correct horse battery staple").unwrap();
        assert!(verify_passphrase("correct horse battery staple", &hash));
        assert!(verify_passphrase("  correct horse battery staple  ", &hash));
        assert!(!verify_passphrase("wrong", &hash));
    }

    /// The stored hash must never be the passphrase, or the whole exercise is decorative.
    #[test]
    fn passphrase_hash_does_not_contain_the_passphrase() {
        let hash = hash_passphrase("hunter2").unwrap();
        assert!(!hash.contains("hunter2"));
    }

    /// Two accounts choosing the same passphrase must not be visibly identical in the database.
    #[test]
    fn passphrases_are_salted() {
        let a = hash_passphrase("same").unwrap();
        let b = hash_passphrase("same").unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn an_empty_stored_hash_never_verifies() {
        assert!(!verify_passphrase("", ""));
        assert!(!verify_passphrase("anything", ""));
        assert!(!verify_passphrase("anything", "not-a-phc-string"));
    }

    #[test]
    fn minted_tokens_are_unique_and_prefixed() {
        let a = mint_token();
        let b = mint_token();
        assert_ne!(a.secret, b.secret);
        assert_eq!(a.prefix.len(), TOKEN_PREFIX_LEN);
        assert!(a.secret.starts_with(&a.prefix));
        assert_eq!(a.hash, hash_token(&a.secret));
        assert_eq!(a.prefix, token_prefix(&a.secret));
        // 32 bytes of entropy, so nothing short enough to be guessable got through.
        assert!(a.secret.len() >= 42, "token too short: {}", a.secret.len());
    }

    #[test]
    fn token_hash_is_not_the_token() {
        let t = mint_token();
        assert!(!t.hash.contains(&t.secret));
        assert_eq!(t.hash.len(), 64);
    }

    #[test]
    fn base64url_is_url_safe() {
        let t = mint_token();
        assert!(
            t.secret.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "token is not URL-safe: {}",
            t.secret
        );
    }

    #[test]
    fn secure_eq_matches_only_identical_strings() {
        assert!(secure_eq("abc", "abc"));
        assert!(!secure_eq("abc", "abd"));
        assert!(!secure_eq("abc", "ab"));
        assert!(secure_eq("", ""));
    }
}
