//! What the social layer must never reveal.
//!
//! Friends are the first deliberate exception to `authorize`, which everywhere else in this server
//! refuses any read of an account that is not the caller's own. An exception is only as good as the
//! things it still refuses, so those are written down here as tests rather than as comments.
//!
//! The invariant every case is an instance of: **a friendship is a door, not a window.** Being
//! someone's friend permits nothing by itself. Each surface — now playing, stats — is gated on its
//! own flag on the subject's account, and every flag defaults closed.

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
    /// Discoverable, and the subject of most of these.
    alpha: Account,
    /// Discoverable. Alpha's friend once a test makes them one.
    beta: Account,
    /// Discoverable, and nobody's friend. The account every refusal is checked against.
    stranger: Account,
}

fn harness() -> Harness {
    let db = Db::new_in_memory().unwrap();
    let alpha = db
        .create_account("alpha", "alpha-pass", Role::Admin, AccountState::Active)
        .unwrap();
    let beta = db
        .create_account("beta", "beta-pass", Role::Member, AccountState::Active)
        .unwrap();
    let stranger = db
        .create_account(
            "stranger",
            "stranger-pass",
            Role::Member,
            AccountState::Active,
        )
        .unwrap();

    // Discoverability defaults closed, which is correct but makes every test start by opting in.
    for who in ["alpha", "beta", "stranger"] {
        db.set_discoverable(who, true).unwrap();
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

    /// Makes two accounts friends the way the API would, request and acceptance both.
    fn befriend(&self, a: &str, b: &str) {
        assert!(
            self.db.send_friend_request(a, b).unwrap(),
            "request {a} -> {b}"
        );
        assert!(
            self.db.accept_friend_request(b, a).unwrap(),
            "accept {b} <- {a}"
        );
    }

    /// Sends a drop through the API and answers with its id.
    async fn drop_a_track(&self, from: &str, to: &str, title: &str) -> String {
        let sender = match from {
            "alpha" => &self.alpha,
            "beta" => &self.beta,
            _ => &self.stranger,
        };
        let sent = self
            .run_as(
                sender,
                &format!(
                    r#"mutation {{ dropTrack(to: "{to}", trackTitle: "{title}", artistName: "Aphex Twin") {{ id }} }}"#
                ),
            )
            .await;
        let body = sent.data.to_string();
        body.split('"')
            .find(|part| part.len() == 36 && part.contains('-'))
            .expect("a uuid in the response")
            .to_string()
    }

    /// Mints a friend code through the API and answers with the code itself.
    async fn mint_friend_code(&self, account: &Account) -> String {
        let minted = self
            .run_as(account, r#"mutation { createFriendCode { code } }"#)
            .await;
        assert_allowed(&minted, "minting a friend code");
        let body = minted.data.to_string();
        body.split('"')
            .find(|part| part.len() > 20 && !part.contains(':'))
            .expect("a code in the response")
            .to_string()
    }

    /// Backdates every outstanding code, standing in for five minutes passing.
    fn expire_friend_codes(&self) {
        let past = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
        self.db
            .conn
            .lock()
            .unwrap()
            .execute("UPDATE friend_codes SET expires_at = ?1", [&past])
            .unwrap();
    }

    /// Puts a track in the server's archive, owned by nobody in particular.
    fn archive_a_track(&self, title: &str) {
        self.db
            .upsert_library_track(&crate::db_library::LibraryTrack {
                content_hash: format!("hash-{title}"),
                title: title.to_string(),
                artist: "Some Artist".into(),
                album: None,
                album_artist: None,
                track_no: None,
                disc_no: None,
                year: None,
                genre: None,
                duration_ms: 1000,
                size_bytes: 1000,
                format: None,
                bitrate_kbps: None,
                archived_path: None,
            })
            .unwrap();
        self.db
            .set_archived_path(&format!("hash-{title}"), "archive/file.flac")
            .unwrap();
    }

    /// Gives an account a listening history to be read out of.
    ///
    /// `ago_secs` places the plays in the past, which is what makes milestones and trendsetter
    /// results testable: both are statements about *when* something happened.
    fn seed_plays(&self, who: &str, title: &str, artist: &str, count: usize, ago_secs: i64) {
        let now = chrono::Utc::now().timestamp();
        let entries: Vec<crate::db::ScrobbleEntry> = (0..count)
            .map(|i| crate::db::ScrobbleEntry {
                track_title: title.to_string(),
                artist_name: artist.to_string(),
                album_name: None,
                genre: None,
                duration_secs: 180,
                // One second apart, so each is a distinct row: without a `play_uid` the scrobble
                // table is unique on (account, artist, title, time) and identical stamps would
                // collapse into one.
                played_at: chrono::DateTime::from_timestamp(now - ago_secs + i as i64, 0)
                    .unwrap()
                    .to_rfc3339(),
                // Deliberately the pre-`play_uid` path. These tests are about *ordering* between
                // accounts — who played a track first — so they need timestamps kept to the
                // second, which is exactly what ingest does for a client that sends no id.
                play_uid: None,
            })
            .collect();
        self.db
            .record_scrobbles(who, "test-device", None, &entries)
            .unwrap();
    }

    /// Gives an account something to be caught playing.
    fn set_playing(&self, who: &str, title: &str) {
        self.set_playing_sealed(who, title, None);
    }

    /// The same, for a session sealed to a set of friend devices.
    ///
    /// `title` is what the plaintext columns carry. A client sealing a session really sends a
    /// placeholder there and the truth in the copies; tests pass a recognisable string instead, so
    /// that a leak of the plaintext column is visible in the assertion rather than indistinguishable
    /// from a correct placeholder.
    fn set_playing_sealed(
        &self,
        who: &str,
        title: &str,
        sealed: Option<&[crate::db_presence::PresenceCiphertext]>,
    ) {
        self.db
            .update_handoff(
                who,
                "track://1",
                title,
                "Some Artist",
                None,
                None,
                0,
                0,
                true,
                "device-1",
                None,
                None,
                None,
                None,
                sealed,
            )
            .unwrap();
    }

    /// One sealed copy, addressed to a device.
    fn copy_for(
        who: &str,
        device: &str,
        ciphertext: &str,
    ) -> crate::db_presence::PresenceCiphertext {
        crate::db_presence::PresenceCiphertext {
            recipient_user_id: who.to_string(),
            recipient_device_id: device.to_string(),
            ciphertext: ciphertext.to_string(),
        }
    }
}

fn assert_refused(response: &async_graphql::Response, what: &str) {
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

// ── The gate itself ─────────────────────────────────────────────────────────────────────────

use crate::db_jam::{JamMode, JamTrackState};

mod wire_contract;

// ── Sealed presence ─────────────────────────────────────────────────────────────────────────
//
// A session sealed to friends' device keys. The invariant under all of these is that a copy is
// addressed: the server holds copies it cannot open and hands each one only to the device it was
// sealed to. The gates that decide *whether* a friend sees a session at all are unchanged and are
// tested above; these assert that sealing did not quietly widen or narrow them.

mod activity_feed;
mod drops_access;
mod drops_receipts;
mod friend_codes;
mod friendship;
mod incognito;
mod invite_spend;
mod invites_and_profile;
mod jam_clock;
mod jam_governance;
mod jam_membership;
mod jam_queue;
mod jam_skip;
mod libraries;
mod recaps;
mod sealed_copies;
mod sealed_friend_list;
mod visibility;
mod wire;
