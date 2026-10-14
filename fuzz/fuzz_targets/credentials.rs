//! Whatever a client presents as a token or a stored hash must be rejected cleanly, never panic.
#![no_main]

use libfuzzer_sys::fuzz_target;

#[path = "../../src/credentials.rs"]
#[allow(dead_code)]
mod credentials;

fuzz_target!(|data: (&str, &str)| {
    let (presented, stored) = data;
    let _ = credentials::verify_passphrase(presented, stored);
    let hashed = credentials::hash_token(presented);
    assert!(credentials::secure_eq(&hashed, &credentials::hash_token(presented)));
    let _ = credentials::token_prefix(presented);
});
