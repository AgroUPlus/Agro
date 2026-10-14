//! The GraphQL schema with everything its resolvers read from the context.

use crate::*;

pub fn build(state: &AppState, setup_token: Arc<auth::SetupToken>) -> AgroSchema {
    let db = state.db.clone();
    let ws_hub = state.ws_hub.clone();
    Schema::build(
        Query::default(),
        Mutation::default(),
        async_graphql::EmptySubscription,
    )
    .data(db.clone())
    .data(ws_hub.clone())
    .data(state.offers.clone())
    // `AppState` itself: `schema_popularity` and `schema_acoustic` resolvers ask the context
    // for the whole struct rather than one field, and nothing else registered it. Without this,
    // every `ctx.data::<AppState>()` in those resolvers fails with "Data `AppState` does not
    // exist" — silently breaking the popularity contributor and acoustic search.
    .data(state.clone())
    // Resolvers need to know whether this deployment archives at all, and where — that is what
    // decides which sync mode the clients are told to run in.
    .data(state.storage.clone())
    .data(setup_token)
    // Per-account ceiling on catalogue publishing. Lives with the schema rather than in
    // `AppState` because the only thing that spends it is a resolver.
    .data(schema_catalog::PublishQuota::default())
    // A public endpoint with no depth or complexity limit is a denial-of-service primitive:
    // GraphQL lets one request ask for a deeply nested or heavily aliased tree, and the cost
    // is paid by the server before any resolver decides the caller was not allowed to ask.
    .limit_depth(12)
    .limit_complexity(500)
    .finish()
}
