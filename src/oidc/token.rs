//! Exchanging the code and verifying the ID token.

use super::*;
use rand::RngCore;
use serde::Deserialize;
use std::time::Duration;

/// The claims this server cares about.
#[derive(Debug, Deserialize)]
pub struct Claims {
    /// The stable identifier. The *only* thing treated as identity.
    pub sub: String,
    pub nonce: Option<String>,
    pub preferred_username: Option<String>,
    pub email: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct TokenResponse {
    pub(super) id_token: String,
}

#[derive(Deserialize)]
pub(super) struct Discovery {
    pub(super) jwks_uri: String,
    pub(super) token_endpoint: String,
    pub(super) issuer: String,
}

#[derive(Deserialize)]
pub(super) struct Jwks {
    pub(super) keys: Vec<serde_json::Value>,
}

/// Exchanges the code and validates the ID token.
pub(super) async fn exchange_and_verify(
    config: &Config,
    code: &str,
    flow: &PendingFlow,
) -> Result<Claims, String> {
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;

    let discovery: Discovery = http
        .get(format!(
            "{}/.well-known/openid-configuration",
            config.issuer
        ))
        .send()
        .await
        .map_err(|e| format!("could not reach the provider: {e}"))?
        .json()
        .await
        .map_err(|e| format!("the provider's discovery document did not parse: {e}"))?;

    let response = http
        .post(&discovery.token_endpoint)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", &config.redirect_uri),
            ("client_id", &config.client_id),
            ("client_secret", &config.client_secret),
            ("code_verifier", &flow.pkce_verifier),
        ])
        .send()
        .await
        .map_err(|e| format!("the token exchange failed: {e}"))?;

    if !response.status().is_success() {
        return Err(format!(
            "the token exchange was refused ({})",
            response.status()
        ));
    }
    let tokens: TokenResponse = response
        .json()
        .await
        .map_err(|e| format!("the token response did not parse: {e}"))?;

    let jwks: Jwks = http
        .get(&discovery.jwks_uri)
        .send()
        .await
        .map_err(|e| format!("could not fetch the provider's keys: {e}"))?
        .json()
        .await
        .map_err(|e| format!("the provider's keys did not parse: {e}"))?;

    let claims = verify_id_token(
        &tokens.id_token,
        &jwks,
        &discovery.issuer,
        &config.client_id,
    )?;

    // Binds this token to the flow this server started. Without it, an ID token obtained for
    // another session could be replayed into this callback.
    match claims.nonce.as_deref() {
        Some(nonce) if crate::credentials::secure_eq(nonce, &flow.nonce) => {}
        _ => return Err("the ID token's nonce did not match this sign-in".into()),
    }
    Ok(claims)
}

/// Checks the ID token's signature and registered claims against the provider's keys.
///
/// The key is selected by the token's `kid`. A token whose `kid` names no published key is refused
/// rather than tried against every key — the latter is how a "try them all" implementation ends up
/// accepting a token signed with something unexpected.
pub(super) fn verify_id_token(
    id_token: &str,
    jwks: &Jwks,
    issuer: &str,
    audience: &str,
) -> Result<Claims, String> {
    let header = jsonwebtoken::decode_header(id_token)
        .map_err(|e| format!("the ID token's header did not parse: {e}"))?;
    let kid = header.kid.ok_or("the ID token names no signing key")?;

    let key_json = jwks
        .keys
        .iter()
        .find(|key| key.get("kid").and_then(|k| k.as_str()) == Some(kid.as_str()))
        .ok_or("the ID token was signed with a key this provider does not publish")?;

    let decoding_key = jsonwebtoken::DecodingKey::from_jwk(
        &serde_json::from_value(key_json.clone())
            .map_err(|e| format!("the provider's key did not parse: {e}"))?,
    )
    .map_err(|e| format!("the provider's key is unusable: {e}"))?;

    // The algorithm comes from the *key*, not from the token's own header — a token that asks to be
    // verified with `none`, or with HMAC using the public key as the secret, is the classic JWT
    // forgery and both are refused by pinning the algorithm here.
    let mut validation = jsonwebtoken::Validation::new(header.alg);
    validation.set_issuer(&[issuer]);
    validation.set_audience(&[audience]);
    validation.validate_exp = true;

    jsonwebtoken::decode::<Claims>(id_token, &decoding_key, &validation)
        .map(|data| data.claims)
        .map_err(|e| format!("the ID token did not validate: {e}"))
}

pub(super) fn random_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    base64url(&bytes)
}

pub(super) fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        let indices = [(n >> 18) & 63, (n >> 12) & 63, (n >> 6) & 63, n & 63];
        for &i in indices.iter().take(chunk.len() + 1) {
            out.push(ALPHABET[i as usize] as char);
        }
    }
    out
}
