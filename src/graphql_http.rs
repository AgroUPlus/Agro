//! The GraphQL endpoint and its companions: SDL, playground and the enrolment-only gate.

use crate::*;

/// Moves the authenticated identity from the request into the GraphQL context.
///
/// `Option`, because `require_token` deliberately lets requests through unauthenticated while the
/// server has no accounts — the window in which the first account is created. Resolvers treat a
/// missing identity as that first-run window; see `schema::authorize`.
/// The operations an administrator who still owes a second factor is allowed to reach.
///
/// Everything else is refused until they finish enrolling. Kept as an allowlist rather than a
/// denylist for the usual reason: a resolver added later is refused by default instead of being
/// quietly exempt because nobody remembered to add it to a list of things to block.
const ENROLMENT_ONLY_OPERATIONS: &[&str] = &[
    "beginTotp",
    "confirmTotp",
    "totpStatus",
    "hasTotp",
    // Reachable so the enrolment screen can greet them by name and know they are an admin.
    "me",
];

/// True when the document asks for nothing outside [`ENROLMENT_ONLY_OPERATIONS`].
///
/// Parsed rather than string-matched: `query { securityEvents }` must not sneak through because the
/// word `totpStatus` appears in a comment or a variable name. An unparseable document is refused
/// here and will fail in the executor anyway.
fn is_enrolment_only(request: &async_graphql::Request) -> bool {
    let Ok(document) = async_graphql::parser::parse_query(&request.query) else {
        return false;
    };
    document.operations.iter().all(|(_, operation)| {
        operation
            .node
            .selection_set
            .node
            .items
            .iter()
            .all(|item| match &item.node {
                async_graphql::parser::types::Selection::Field(field) => {
                    ENROLMENT_ONLY_OPERATIONS.contains(&field.node.name.node.as_str())
                }
                // A fragment or inline spread at the top level could name anything, and resolving
                // it here would mean reimplementing the executor. Refused.
                _ => false,
            })
    })
}

/// `GET /graphql/sdl` — the schema definition language, for anyone who wants a typed client.
pub async fn graphql_sdl(schema: axum::Extension<AgroSchema>) -> impl axum::response::IntoResponse {
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; charset=utf-8",
        )],
        schema.sdl(),
    )
}

/// `GET /graphql/playground` — a GraphiQL client wired to `/graphql`, for exploring the schema by
/// hand. A real bearer token still has to be typed into its headers panel to query anything.
pub async fn graphql_playground() -> axum::response::Html<String> {
    axum::response::Html(
        async_graphql::http::GraphiQLSource::build()
            .endpoint("/graphql")
            .finish(),
    )
}

pub async fn graphql_handler(
    schema: axum::Extension<AgroSchema>,
    State(state): State<AppState>,
    user: Option<axum::Extension<auth::AuthedUser>>,
    req: GraphQLRequest,
) -> GraphQLResponse {
    let mut request = req.into_inner();
    if let Some(axum::Extension(user)) = user {
        // An administrator on a server that requires a second factor may do exactly one thing until
        // they have one. The check lives here rather than in the token middleware because the
        // middleware does not parse GraphQL — and should not start.
        let owes_enrolment = user.is_admin()
            && auth::admin_totp_required()
            && !state.db.totp_is_confirmed(user.username()).unwrap_or(false);

        if owes_enrolment && !is_enrolment_only(&request) {
            // Carries a machine-readable code as well as prose. The dashboard sends composite
            // documents — one query fetches the account, the devices and the current track
            // together — so this refusal lands on every screen at once, and with nothing to
            // recognise it by, the client can only render a dashboard whose every field is empty.
            // That is exactly what it did: it looked broken rather than like a step to take.
            let mut error = async_graphql::ServerError::new(
                "Set up two-factor authentication to finish signing in. \
                 This server requires it for administrators.",
                None,
            );
            let mut extensions = async_graphql::ErrorExtensionValues::default();
            extensions.set("code", "TOTP_ENROLMENT_REQUIRED");
            error.extensions = Some(extensions);
            return async_graphql::Response::from_errors(vec![error]).into();
        }
        request = request.data(user);
    }
    schema.execute(request).await.into()
}
