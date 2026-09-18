# Security & Zero-Knowledge Vault

## 1. Zero-Knowledge Settings Vault

- The server holds `vault_salt` and `vault_key_wrapped` sealed under Argon2id(passphrase).
- **CRITICAL INVARIANT**: The server must never receive, derive, or store plaintext or master keys that could unwrap the user's settings vault.

## 2. Sealed Presence & Handoff

- Telemetry and metadata in transit across friends or devices is end-to-end sealed using asymmetric cryptography.
- The Agro relay daemon routes ciphertext payloads it cannot inspect or read.

## 3. Credential & Hash Storage

- User passphrases are stored strictly as **Argon2id** password hashes.
- Device authorization tokens are stored exclusively as **SHA-256** cryptographic hashes.
- Device tokens are revocable and expire automatically after a period of inactivity.
- The external archive hook receives execution parameters via environment variables, never through shell command string interpolation.
- Zero analytics, telemetry trackers, or phone-home dependencies.
