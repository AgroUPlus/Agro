//! What the shared catalogue does and does not expose between accounts.
//!
//! The catalogue is **cross-account by construction**: none of its tables carries an account
//! column, and `catalogSince` filters on a timestamp alone. That is the point of it — the work of
//! identifying a recording is done once for everyone — but it is also the only cross-account path
//! here without a written-down guarantee. `db_popularity` has an exposure floor and day-bucket
//! retention; `db_acoustic` states the no-account-column invariant in its own header. This file is
//! the catalogue's equivalent: the sharing is asserted rather than implied, so that a future change
//! narrowing or widening it has to change a test that says what it meant.
//!
//! The one thing deliberately *not* shared is a `local:` source id. Those are filesystem paths from
//! somebody's phone, and `sources` is handed to every account on the server.

#![cfg(test)]

use crate::auth::{AuthedUser, SetupToken};
use crate::db::Db;
use crate::db_catalog::quantise;
use crate::db_identity::{Account, AccountState, Role};
use crate::schema::{AgroSchema, Mutation, Query};
use crate::storage::Storage;
use crate::ws::WsHub;
use async_graphql::{Request, Schema};
use std::sync::Arc;

const DIM: usize = 128;

struct Harness {
    schema: AgroSchema,
    alpha: Account,
    beta: Account,
}

fn harness() -> Harness {
    let db = Db::new_in_memory().unwrap();
    let alpha = db
        .create_account("alpha", "alpha-pass", Role::Member, AccountState::Active)
        .unwrap();
    let beta = db
        .create_account("beta", "beta-pass", Role::Member, AccountState::Active)
        .unwrap();

    let schema = Schema::build(
        Query::default(),
        Mutation::default(),
        async_graphql::EmptySubscription,
    )
    .data(db)
    .data(Arc::new(WsHub::new()))
    .data(Storage::for_tests())
    .data(SetupToken::for_fresh_server(1))
    .data(crate::schema_catalog::PublishQuota::default())
    .finish();

    Harness {
        schema,
        alpha,
        beta,
    }
}

impl Harness {
    async fn run_as(&self, account: &Account, query: &str) -> async_graphql::Response {
        self.schema
            .execute(Request::new(query).data(AuthedUser {
                account: account.clone(),
                device_label: String::new(),
                token_hash: String::new(),
            }))
            .await
    }
}

/// A 32-bit finaliser, so one changed input bit changes half the output bits.
fn mix(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    x
}

/// A hex int8 embedding, as the wire carries it.
fn embedding_hex(seed: u32) -> String {
    let bytes: Vec<u8> = (0..60u32)
        .flat_map(|segment| {
            let mut v: Vec<f32> = (0..DIM as u32)
                .map(|d| (mix(seed ^ mix(segment ^ mix(d))) as f32 / u32::MAX as f32) - 0.5)
                .collect();
            let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
            for x in v.iter_mut() {
                *x /= norm;
            }
            quantise(&v)
        })
        .collect();
    hex::encode(bytes)
}

fn publish(embedding: &str, title: &str, source: &str) -> String {
    format!(
        r#"mutation {{ publishRecording(
             embedding: "{embedding}", dim: 128, model: "nmfp-triplet", version: 1,
             durationMs: 210000, title: "{title}", artist: "An Artist", sourceUri: "{source}"
           ) }}"#
    )
}

// ── The sharing this exists for ─────────────────────────────────────────────────────────────

/// One entry of a `publishRecordings` batch, as GraphQL literal syntax.
fn entry(embedding: &str, title: &str, source: &str) -> String {
    format!(
        r#"{{ embedding: "{embedding}", dim: 128, model: "nmfp-triplet", version: 1,
             durationMs: 210000, title: "{title}", artist: "An Artist", sourceUri: "{source}" }}"#
    )
}

mod batches;
mod lyrics_and_subscriptions;
mod publishing;
