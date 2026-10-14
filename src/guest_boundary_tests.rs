//! What a hostile guest account cannot do.
//!
//! Every case here is a thing that *worked* before the admin/guest boundary existed. They are kept
//! together, and phrased as the attack rather than as the feature, because the failure mode being
//! guarded against is a future change quietly re-opening one of them: a resolver that stops calling
//! `require_admin`, a query that goes back to trusting a caller-supplied device id.
//!
//! These drive the real schema through `Schema::execute`, so they exercise the same resolvers and
//! the same guards a request would. Three things they deliberately do *not* cover, because none of
//! them are reachable from a schema execution:
//!
//! - the middleware in `auth`, which is where a token becomes an identity;
//! - `login` and `bootstrap`, which are unauthenticated REST routes (see `crate::login`) precisely
//!   so that `/graphql` never has to accept an anonymous caller;
//! - the streaming quota enforcement in `library::put_upload`, which needs a live request body.
//!
//! Those are verified against a running server instead.

#![cfg(test)]

use crate::auth::{AuthedUser, SetupToken};
use crate::db::Db;
use crate::db_identity::{Account, AccountState, Role};
use crate::schema::{AgroSchema, Mutation, Query};
use crate::storage::Storage;
use crate::ws::WsHub;
use async_graphql::{Request, Schema};
use std::sync::Arc;

struct Harness {
    schema: AgroSchema,
    db: Db,
    admin: Account,
    guest: Account,
}

fn harness() -> Harness {
    let db = Db::new_in_memory().unwrap();
    let admin = db
        .create_account("alpha", "admin-pass", Role::Admin, AccountState::Active)
        .unwrap();
    let guest = db
        .create_account("mallory", "guest-pass", Role::Member, AccountState::Active)
        .unwrap();

    let storage = Storage::for_tests();
    let schema = Schema::build(
        Query::default(),
        Mutation::default(),
        async_graphql::EmptySubscription,
    )
    .data(db.clone())
    .data(Arc::new(WsHub::new()))
    .data(storage)
    .data(SetupToken::for_fresh_server(1))
    .finish();

    Harness {
        schema,
        db,
        admin,
        guest,
    }
}

impl Harness {
    /// Runs a document as an account, exactly as an authenticated request would.
    async fn run_as(&self, account: &Account, query: &str) -> async_graphql::Response {
        let request = Request::new(query).data(AuthedUser {
            account: account.clone(),
            device_label: String::new(),
            token_hash: String::new(),
        });
        self.schema.execute(request).await
    }

    /// Runs a document with no identity at all — a request that got past the middleware without
    /// one, which is what the old fail-open `authorize` allowed.
    async fn run_anonymously(&self, query: &str) -> async_graphql::Response {
        self.schema.execute(Request::new(query)).await
    }
}

fn assert_forbidden(response: &async_graphql::Response, what: &str) {
    assert!(
        !response.errors.is_empty(),
        "{what}: expected a refusal, got data: {:?}",
        response.data
    );
}

fn assert_allowed(response: &async_graphql::Response, what: &str) {
    assert!(
        response.errors.is_empty(),
        "{what}: expected success, got errors: {:?}",
        response.errors
    );
}

// ── The account boundary ────────────────────────────────────────────────────────────────────

mod devices;
mod erasure;
mod identity;
mod security_log;
mod server_state;
mod sessions;
