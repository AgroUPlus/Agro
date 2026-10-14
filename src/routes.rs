//! The HTTP router: which routes exist, which are public, and the layers around them.

use crate::graphql_http::{graphql_handler, graphql_playground, graphql_sdl};
use crate::security_headers::security_headers;
use crate::*;

pub fn cors_from_env() -> CorsLayer {
    // The dashboard is served from this same origin, so a wildcard buys nothing and lets any page
    // on the internet make authenticated-looking requests from a visitor's browser.
    match std::env::var("AGRO_ALLOWED_ORIGIN") {
        Ok(origin) if !origin.trim().is_empty() => match origin.trim().parse::<HeaderValue>() {
            Ok(value) => CorsLayer::new()
                .allow_origin(value)
                .allow_methods(Any)
                .allow_headers(Any),
            Err(_) => {
                tracing::error!("AGRO_ALLOWED_ORIGIN is not a valid origin; refusing all CORS");
                CorsLayer::new()
            }
        },
        _ => CorsLayer::new(),
    }
}

pub fn build(state: AppState, schema: AgroSchema) -> Router {
    let db = state.db.clone();
    let cors = cors_from_env();
    // Each switchable feature's routes refuse while it is off. See `features::gate`.
    let gated = |feature: Feature| {
        axum::middleware::from_fn_with_state((db.clone(), feature), features::gate)
    };

    // Everything that exposes a user's data sits behind the token check; the dashboard's own
    // static files and the capability-URL share endpoint stay public.
    let protected = Router::new()
        .route("/graphql", post(graphql_handler))
        .route("/ws/sync", get(ws::ws_handler))
        .route(
            "/api/v1/library/upload",
            post(library::begin_upload).layer(gated(Feature::LibraryTransfers)),
        )
        .route(
            "/api/v1/library/upload/{upload_id}",
            put(library::put_upload)
                .layer(DefaultBodyLimit::disable())
                .layer(gated(Feature::LibraryTransfers)),
        )
        .route(
            "/api/v1/library/fetch/{content_hash}",
            get(library::fetch).layer(gated(Feature::LibraryTransfers)),
        )
        .route(
            "/api/v1/vault/backups",
            put(vault_http::put_backup)
                // Room for the largest sealed backup and nothing else; the server-wide limit is
                // for JSON and would refuse a real one.
                .layer(DefaultBodyLimit::max(db_vault::MAX_SEALED_BYTES))
                .layer(gated(Feature::CloudBackups)),
        )
        .route(
            "/api/v1/vault/backups/{id}",
            get(vault_http::get_backup).layer(gated(Feature::CloudBackups)),
        )
        .route("/api/v1/cover/{album_key}", get(library::cover))
        .route(
            "/api/v1/relay/open",
            post(relay::open_relay).layer(gated(Feature::AudioRelay)),
        )
        .route(
            "/api/v1/relay/{session_id}/send",
            post(relay::send_relay)
                .layer(DefaultBodyLimit::disable())
                .layer(gated(Feature::AudioRelay)),
        )
        .route(
            "/api/v1/relay/{session_id}/receive",
            get(relay::receive_relay).layer(gated(Feature::AudioRelay)),
        )
        .route(
            "/api/v1/proxy",
            axum::routing::any(proxy::proxy_handler).layer(gated(Feature::PrivacyProxy)),
        )
        .route("/api/v1/oidc/link", get(oidc::start_link))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_token,
        ));

    // `/api/v1/dropbox/upload` used to be routed here, *outside* `protected` — an unauthenticated
    // endpoint that joined a caller-supplied filename onto a path (so `../..` escaped the upload
    // directory), buffered whole files in RAM on a 512 MB host, and wrote nothing to the database.
    // It is replaced by the authenticated streaming routes in `library`, which live inside
    // `protected` above.
    Router::new()
        .merge(protected)
        // The only two routes that can be reached without a token: there has to be some way to
        // get one. Both are rate-limited; see `login`.
        .route("/api/v1/login", post(login::login))
        .route("/api/v1/setup-status", get(setup_status::setup_status))
        .route("/api/v1/bootstrap", post(login::bootstrap))
        .route("/api/v1/signup", post(login::signup))
        // SSO. `config` and `start` have to be reachable by someone with no account yet, and
        // `callback` is where the provider sends the browser back. `link` is the only one that
        // needs an identity, and it lives on the protected router above.
        .route("/api/v1/oidc/config", get(oidc::config))
        .route("/api/v1/oidc/start", get(oidc::start))
        .route("/api/v1/oidc/callback", get(oidc::callback))
        .route("/share/{token}", get(share::share_handler))
        // Public by design: a shared link is opened by someone with no account here.
        .route("/listen", get(listen::listen_handler))
        // Public by design, for the docs site's logged-out Charts page — see `popular`'s module
        // doc for why this discloses nothing the GraphQL query's auth check was protecting. A
        // route-scoped `Any`-origin CORS layer rather than widening the global `cors` layer below,
        // which is deliberately single-origin and guards every *authenticated* route.
        .route(
            "/api/v1/popular",
            get(popular::popular_handler)
                .layer(CorsLayer::new().allow_origin(Any).allow_methods(Any)),
        )
        // Documentation, not data: safe to leave public. GraphQL's SDL describes the shape of the
        // schema, not any account's contents, and GraphiQL still needs a real bearer token typed
        // into its headers panel before it can query anything.
        .route("/graphql/sdl", get(graphql_sdl))
        .route("/graphql/playground", get(graphql_playground))
        .merge(
            SwaggerUi::new("/api/docs").url("/api/docs/openapi.json", openapi::ApiDoc::openapi()),
        )
        .fallback(embedded_dashboard::static_dashboard_handler)
        // Without this a single request can stream unbounded bytes into any JSON handler. The
        // upload routes opt back out, because that is exactly what they are for.
        .layer(DefaultBodyLimit::max(MAX_JSON_BODY_BYTES))
        .layer(axum::middleware::from_fn(security_headers))
        .layer(cors)
        .with_state(state)
        .layer(axum::Extension(schema))
}
