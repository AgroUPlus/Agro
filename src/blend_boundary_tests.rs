//! What a Blend must never let through, tested over GraphQL.
//!
//! A blend is made of people's listening, so its rules are consent rules: nobody is read before
//! they accept, nobody outside it can open it, and nobody — its creator included — can write over
//! what Agro writes.

#![cfg(test)]

use crate::auth::{AuthedUser, SetupToken};
use crate::db::{Db, ScrobbleEntry};
use crate::db_identity::{Account, AccountState, Role};
use crate::schema::{AgroSchema, Mutation, Query};
use crate::storage::Storage;
use crate::ws::WsHub;
use async_graphql::{Request, Schema};
use serde_json::Value;
use std::sync::Arc;

pub(crate) struct Harness {
    schema: AgroSchema,
    pub(crate) db: Db,
    pub(crate) alpha: Account,
    pub(crate) beta: Account,
    /// alpha's friend, never asked into anything.
    gamma: Account,
    stranger: Account,
}

pub(crate) fn harness() -> Harness {
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
    let (alpha, beta, gamma, stranger) = (
        account("alpha"),
        account("beta"),
        account("gamma"),
        account("stranger"),
    );
    for friend in ["beta", "gamma"] {
        assert!(db.send_friend_request(friend, "alpha").unwrap());
        assert!(db.accept_friend_request("alpha", friend).unwrap());
    }
    {
        let conn = db.conn.lock().unwrap();
        conn.execute("UPDATE users SET show_stats = 1", []).unwrap();
    }
    for who in ["alpha", "beta", "gamma"] {
        let plays: Vec<ScrobbleEntry> = (0..5)
            .map(|i| ScrobbleEntry {
                track_title: format!("{who} song {i}"),
                artist_name: format!("{who} artist {i}"),
                album_name: None,
                genre: None,
                duration_secs: 200,
                played_at: chrono::Utc::now().to_rfc3339(),
                play_uid: Some(format!("{who}-{i}")),
            })
            .collect();
        db.record_scrobbles(who, "phone", None, &plays).unwrap();
    }
    let schema = Schema::build(
        Query::default(),
        Mutation::default(),
        async_graphql::EmptySubscription,
    )
    .data(db.clone())
    .data(Arc::new(WsHub::new()))
    .data(Storage::for_tests())
    .data(SetupToken::for_fresh_server(1))
    .finish();
    Harness {
        schema,
        db,
        alpha,
        beta,
        gamma,
        stranger,
    }
}

impl Harness {
    async fn run(&self, account: &Account, query: &str) -> async_graphql::Response {
        let request = Request::new(query).data(AuthedUser {
            account: account.clone(),
            device_label: String::new(),
            token_hash: String::new(),
        });
        self.schema.execute(request).await
    }

    pub(crate) async fn ok(&self, account: &Account, query: &str) -> Value {
        let response = self.run(account, query).await;
        assert!(response.errors.is_empty(), "refused: {:?}", response.errors);
        response.data.into_json().unwrap()
    }

    async fn refused(&self, account: &Account, query: &str, why: &str) {
        let response = self.run(account, query).await;
        assert!(!response.errors.is_empty(), "{why}: it was allowed");
    }

    /// alpha makes a blend asking beta, and returns its id.
    async fn blend(&self) -> String {
        let made = self
            .ok(
                &self.alpha,
                r#"mutation { createBlend(title: "Us", members: ["beta"], size: 25, mix: 50,
                     window: ALL_TIME, refresh: WEEKLY) { playlistId } }"#,
            )
            .await;
        made["createBlend"]["playlistId"]
            .as_str()
            .unwrap()
            .to_string()
    }
}

fn open_query(id: &str) -> String {
    format!(r#"{{ playlist(id: "{id}") {{ isBlend myRole items {{ title addedBy }} }} }}"#)
}

mod lifecycle;
mod visibility;
