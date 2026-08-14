use aes_gcm::{
    aead::{Aead, KeyInit, OsRng},
    Aes256Gcm, Nonce,
};
use sha2::{Digest, Sha256};

const SALT: &[u8] = b"agro-vault-v1-encryption-salt";

/// Derive a 256-bit AES key from the user's 4-word natural passphrase.
pub fn derive_key(passphrase: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(SALT);
    hasher.update(passphrase.trim().as_bytes());
    let first_round = hasher.finalize();

    let mut second_hasher = Sha256::new();
    second_hasher.update(first_round);
    second_hasher.update(SALT);
    let mut key = [0u8; 32];
    key.copy_from_slice(&second_hasher.finalize());
    key
}

/// Encrypt plaintext using AES-256-GCM with a fresh random 96-bit nonce.
/// Returns hex-encoded `nonce (12 bytes) || ciphertext_with_tag`.
pub fn encrypt_field(plaintext: &str, passphrase: &str) -> Result<String, String> {
    if plaintext.is_empty() {
        return Ok(String::new());
    }
    let key = derive_key(passphrase);
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|e| e.to_string())?;
    
    use aes_gcm::aead::rand_core::RngCore;
    let mut nonce_bytes = [0u8; 12];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext.as_bytes())
        .map_err(|e| e.to_string())?;

    let mut combined = Vec::with_capacity(nonce_bytes.len() + ciphertext.len());
    combined.extend_from_slice(&nonce_bytes);
    combined.extend_from_slice(&ciphertext);

    Ok(hex::encode(combined))
}

/// Decrypt hex-encoded `nonce || ciphertext_with_tag` using the passphrase.
pub fn decrypt_field(ciphertext_hex: &str, passphrase: &str) -> Result<String, String> {
    if ciphertext_hex.is_empty() {
        return Ok(String::new());
    }
    // If not valid hex or too short, treat as unencrypted legacy data
    let bytes = match hex::decode(ciphertext_hex) {
        Ok(b) if b.len() > 12 => b,
        _ => return Ok(ciphertext_hex.to_string()),
    };

    let (nonce_bytes, ciphertext) = bytes.split_at(12);
    let key = derive_key(passphrase);
    let cipher = match Aes256Gcm::new_from_slice(&key) {
        Ok(c) => c,
        Err(_) => return Ok(ciphertext_hex.to_string()),
    };
    let nonce = Nonce::from_slice(nonce_bytes);

    match cipher.decrypt(nonce, ciphertext) {
        Ok(decrypted) => String::from_utf8(decrypted).map_err(|e| e.to_string()),
        Err(_) => {
            // Fallback to raw value if passphrase doesn't match or was plain
            Ok(ciphertext_hex.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let passphrase = "amber-shield-orbit-matrix";
        let secret = "my_super_secret_subsonic_token_12345";

        let encrypted = encrypt_field(secret, passphrase).expect("encryption succeeds");
        assert_ne!(encrypted, secret);

        let decrypted = decrypt_field(&encrypted, passphrase).expect("decryption succeeds");
        assert_eq!(decrypted, secret);

        // Wrong passphrase fails to decrypt cleanly
        let wrong_decrypted = decrypt_field(&encrypted, "wrong-passphrase-here-now").unwrap();
        assert_ne!(wrong_decrypted, secret);
    }
}
