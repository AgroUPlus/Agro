//! The generated OpenAPI document for the REST surface.
//!
//! GraphQL is the bulk of the API and is documented separately — its SDL is served at
//! `/graphql/sdl` and it is explorable at `/graphql/playground`. This document covers only the
//! handful of REST endpoints that exist because GraphQL is a poor fit for them: authentication
//! bootstrapping, binary transfer, device relay, SSO redirects and share links.
//!
//! Every `#[utoipa::path]` annotation lives next to the handler it describes, in that handler's own
//! file, so the two cannot drift apart the way a hand-maintained document would. This file only
//! collects them into one [`ApiDoc`] and defines the handful of types that exist purely to describe
//! responses that are built ad hoc with `serde_json::json!(...)` rather than through a typed struct.

use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi, ToSchema};

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Agro API",
        version = env!("CARGO_PKG_VERSION"),
        description = "REST surface of the Agro media server. This is a small, deliberately \
                        non-GraphQL slice of the API — see /graphql/sdl for the schema definition \
                        language and /graphql/playground for an interactive GraphQL client, which \
                        together cover almost everything else this server does.\n\n\
                        Authentication: most routes take a bearer device token, normally as an \
                        `Authorization: Bearer <token>` header (the only form modeled below). The \
                        same token is also accepted as a `?token=` query parameter or a `token=` \
                        cookie, for the WebSocket and browser-navigation cases that cannot set a \
                        header; those two forms are not shown in this document's \"Authorize\" \
                        dialog but work identically.",
    ),
    paths(
        crate::login::login,
        crate::login::bootstrap,
        crate::login::signup,
        crate::library::begin_upload,
        crate::library::put_upload,
        crate::library::fetch,
        crate::library::cover,
        crate::relay::open_relay,
        crate::relay::send_relay,
        crate::relay::receive_relay,
        crate::oidc::config,
        crate::oidc::start,
        crate::oidc::start_link,
        crate::oidc::callback,
        crate::proxy::proxy_handler,
        crate::share::share_handler,
        crate::listen::listen_handler,
        crate::popular::popular_handler,
    ),
    components(schemas(
        crate::login::LoginBody,
        LoginResponse,
        crate::login::BootstrapBody,
        BootstrapResponse,
        crate::login::SignupBody,
        SignupResponse,
        crate::library::BeginUpload,
        crate::library::BeginUploadResponse,
        crate::relay::OpenRelayRequest,
        crate::relay::OpenRelayResponse,
        crate::popular::PopularResponse,
        crate::popular::PopularTrackJson,
        ApiError,
    )),
    tags(
        (name = "auth", description = "The two ways to get a bearer token: signing in, and the \
                                        one-time server bootstrap/signup that come before it."),
        (name = "library", description = "Streaming file upload and download. Kept as REST rather \
                                           than GraphQL because these bodies carry megabytes and a \
                                           base64 JSON envelope would both inflate and buffer them."),
        (name = "relay", description = "Zero-disk device-to-device audio relay, used when two of \
                                         an account's devices cannot reach each other directly."),
        (name = "oidc", description = "Single sign-on via an external OIDC provider."),
        (name = "sharing", description = "Public, unauthenticated link handlers. `/listen` and its \
                                           parameters are specified in SHARE_LINKS.md and shared \
                                           with external Kotlin/JS clients — treat the wire shape \
                                           as frozen."),
        (name = "popular", description = "Public, unauthenticated fleet-wide chart data — the same \
                                           no-auth precedent as \"sharing\", for a logged-out \
                                           caller such as the docs site's Charts page."),
        (name = "proxy", description = "Outbound passthrough proxy to a small allow-list of \
                                         metadata/lyrics hosts, so the browser never talks to them \
                                         directly."),
    ),
    modifiers(&SecurityAddon),
)]
pub struct ApiDoc;

struct SecurityAddon;

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        if let Some(components) = openapi.components.as_mut() {
            components.add_security_scheme(
                "bearer_token",
                SecurityScheme::Http(
                    HttpBuilder::new()
                        .scheme(HttpAuthScheme::Bearer)
                        .bearer_format("device token")
                        .build(),
                ),
            );
        }
    }
}

/// The shape of every hand-built REST error body: `{"error": "..."}`, with an extra
/// `totpRequired` flag on the two login responses that involve a second factor.
///
/// Purely documentation — this struct is never constructed at runtime. Handlers keep building
/// `serde_json::json!({"error": ...})` directly, so this cannot drift from what they actually send.
#[derive(serde::Serialize, ToSchema)]
pub struct ApiError {
    error: String,
    /// Present only on `POST /api/v1/login`'s two second-factor responses.
    #[serde(skip_serializing_if = "Option::is_none")]
    totp_required: Option<bool>,
}

/// Shape of `login`'s JSON success body. Documentation-only; see [`ApiError`].
#[derive(serde::Serialize, ToSchema)]
pub struct LoginResponse {
    username: String,
    role: String,
    token: String,
    vault_salt: Option<String>,
    vault_key_wrapped: Option<String>,
    totp_enrolment_required: bool,
}

/// Shape of `bootstrap`'s JSON success body. Documentation-only; see [`ApiError`].
#[derive(serde::Serialize, ToSchema)]
pub struct BootstrapResponse {
    username: String,
    /// Shown once — the server keeps only an Argon2 hash and cannot produce this again.
    passphrase: String,
    token: String,
}

/// Shape of `signup`'s JSON success body. Documentation-only; see [`ApiError`].
#[derive(serde::Serialize, ToSchema)]
pub struct SignupResponse {
    username: String,
    state: String,
    /// Shown once — the server keeps only an Argon2 hash and cannot produce this again.
    passphrase: String,
}
