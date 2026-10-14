//! What shared and collaborative playlists must never let through, tested over GraphQL.
//!
//! The database tests prove the rules; these prove the API does not route around them: that an
//! id cannot be probed, that a push carries nothing but an id and a revision, and that a stale
//! edit comes back with a code a client can act on.

#![cfg(test)]

use crate::auth::{AuthedUser, SetupToken};
use crate::db::Db;
use crate::db_identity::{Account, AccountState, Role};
use crate::playlist_visibility::PlaylistVisibility;
use crate::schema::{AgroSchema, Mutation, Query};
use crate::storage::Storage;
use crate::ws::WsHub;
use async_graphql::{Request, Schema};
use serde_json::Value;
use std::sync::Arc;

struct Harness {
    schema: AgroSchema,
    db: Db,
    hub: Arc<WsHub>,
    alpha: Account,
    beta: Account,
    stranger: Account,
}

fn harness() -> Harness {
    let db = Db::new_in_memory().unwrap();
    let account = |name: &str| {
        db.create_account(
            name,
            &format!("{name}-pass"),
            Role::Member,
            AccountState::Active,
        )
        .unwrap()
    };
    let (alpha, beta, stranger) = (account("alpha"), account("beta"), account("stranger"));
    assert!(db.send_friend_request("beta", "alpha").unwrap());
    assert!(db.accept_friend_request("alpha", "beta").unwrap());

    let hub = Arc::new(WsHub::new());
    let schema = Schema::build(
        Query::default(),
        Mutation::default(),
        async_graphql::EmptySubscription,
    )
    .data(db.clone())
    .data(hub.clone())
    .data(Storage::for_tests())
    .data(SetupToken::for_fresh_server(1))
    .finish();
    Harness {
        schema,
        db,
        hub,
        alpha,
        beta,
        stranger,
    }
}

impl Harness {
    async fn run_as(&self, account: &Account, query: &str) -> async_graphql::Response {
        let request = Request::new(query).data(AuthedUser {
            account: account.clone(),
            device_label: String::new(),
            token_hash: String::new(),
        });
        self.schema.execute(request).await
    }

    fn playlist(&self, visibility: PlaylistVisibility) -> String {
        self.db
            .create_playlist("alpha", "Mix", None, visibility)
            .unwrap()
            .id
    }
}

fn data(response: async_graphql::Response) -> Value {
    assert!(
        response.errors.is_empty(),
        "unexpected errors: {:?}",
        response.errors
    );
    response.data.into_json().unwrap()
}

const ADD: &str = r#"add: { track: { title: "Song", artist: "Artist" } }"#;

mod access;
mod followers;
