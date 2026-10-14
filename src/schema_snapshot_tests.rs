//! The GraphQL schema as a committed file, so an API change is a visible diff in review.
//!
//! Wanda and Wander are typed clients of this schema. A renamed field or a tightened argument
//! breaks them on a phone nobody can hot-fix, so the schema is not allowed to move silently.
//!
//! After an intentional change, regenerate with `UPDATE_SNAPSHOT=1 cargo test schema_snapshot`.

use crate::{Mutation, Query};
use async_graphql::{EmptySubscription, Schema};

const SNAPSHOT: &str = "docs/api/schema.graphql";

#[test]
fn the_graphql_schema_matches_the_committed_snapshot() {
    let current = Schema::build(Query::default(), Mutation::default(), EmptySubscription)
        .finish()
        .sdl();
    if std::env::var_os("UPDATE_SNAPSHOT").is_some() {
        std::fs::write(SNAPSHOT, &current).expect("snapshot is writable");
        return;
    }
    let committed = std::fs::read_to_string(SNAPSHOT)
        .expect("docs/api/schema.graphql is missing; run with UPDATE_SNAPSHOT=1");
    assert!(
        committed == current,
        "the GraphQL schema changed; review the diff, then regenerate with UPDATE_SNAPSHOT=1"
    );
}
