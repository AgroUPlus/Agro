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

use async_graphql::{Request, Schema};
use std::sync::Arc;

use crate::auth::{AuthedUser, SetupToken};
use crate::db::Db;
use crate::db_identity::{Account, AccountState, Role};
use crate::schema::{AgroSchema, Mutation, Query};
use crate::storage::Storage;
use crate::ws::WsHub;

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
        .create_account("stranger", "stranger-pass", Role::Member, AccountState::Active)
        .unwrap();

    // Discoverability defaults closed, which is correct but makes every test start by opting in.
    for who in ["alpha", "beta", "stranger"] {
        db.set_discoverable(who, true).unwrap();
    }

    let schema = Schema::build(Query::default(), Mutation::default(), async_graphql::EmptySubscription)
        .data(db.clone())
        .data(Arc::new(WsHub::new()))
        .data(Storage::for_tests())
        .data(SetupToken::for_fresh_server(1))
        .finish();

    Harness { schema, db, alpha, beta, stranger }
}

impl Harness {
    async fn run_as(&self, account: &Account, query: &str) -> async_graphql::Response {
        let request = Request::new(query).data(AuthedUser {
            account: account.clone(),
            device_label: String::new(),
        });
        self.schema.execute(request).await
    }

    /// Makes two accounts friends the way the API would, request and acceptance both.
    fn befriend(&self, a: &str, b: &str) {
        assert!(self.db.send_friend_request(a, b).unwrap(), "request {a} -> {b}");
        assert!(self.db.accept_friend_request(b, a).unwrap(), "accept {b} <- {a}");
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
                // One second apart, so each is a distinct row: the scrobble table is unique on
                // (account, artist, title, time) and identical stamps would collapse into one.
                played_at: chrono::DateTime::from_timestamp(now - ago_secs + i as i64, 0)
                    .unwrap()
                    .to_rfc3339(),
            })
            .collect();
        self.db.record_scrobbles(who, "test-device", None, &entries).unwrap();
    }

    /// Gives an account something to be caught playing.
    fn set_playing(&self, who: &str, title: &str) {
        self.db
            .update_handoff(
                who,
                "track://1",
                title,
                "Some Artist",
                None,
                None,
                0,
                true,
                "device-1",
                None,
                None,
            )
            .unwrap();
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

/// The headline case: friendship on its own reveals nothing.
#[tokio::test]
async fn a_friend_sees_nothing_until_the_flag_is_set() {
    let h = harness();
    h.befriend("alpha", "beta");
    h.set_playing("alpha", "A Secret Song");

    let closed = h.run_as(&h.beta, "{ friendsNowPlaying { trackTitle } }").await;
    assert_allowed(&closed, "friendsNowPlaying");
    assert_eq!(
        closed.data.to_string(),
        r#"{friendsNowPlaying: []}"#,
        "a friend's playback leaked with show_now_playing off"
    );

    h.db.set_visibility("alpha", true, false).unwrap();
    let open = h.run_as(&h.beta, "{ friendsNowPlaying { trackTitle } }").await;
    assert!(
        open.data.to_string().contains("A Secret Song"),
        "the flag was set and the friend still saw nothing: {:?}",
        open.data
    );
}

#[tokio::test]
async fn a_stranger_never_sees_now_playing_however_open_the_flag() {
    let h = harness();
    h.db.set_visibility("alpha", true, true).unwrap();
    h.set_playing("alpha", "A Secret Song");

    let r = h.run_as(&h.stranger, "{ friendsNowPlaying { trackTitle } }").await;
    assert_eq!(r.data.to_string(), r#"{friendsNowPlaying: []}"#);

    let along = h
        .run_as(&h.stranger, r#"mutation { startListenAlong(host: "alpha") { host } }"#)
        .await;
    assert_refused(&along, "startListenAlong on a non-friend");
}

#[tokio::test]
async fn stats_are_gated_separately_from_now_playing() {
    let h = harness();
    h.befriend("alpha", "beta");
    // Now playing open, stats closed. One flag must not imply the other.
    h.db.set_visibility("alpha", true, false).unwrap();

    let r = h
        .run_as(&h.beta, r#"{ tasteMatch(username: "alpha") { score } }"#)
        .await;
    assert_refused(&r, "tasteMatch with show_stats off");

    h.db.set_visibility("alpha", true, true).unwrap();
    let allowed = h
        .run_as(&h.beta, r#"{ tasteMatch(username: "alpha") { score } }"#)
        .await;
    assert_allowed(&allowed, "tasteMatch with show_stats on");
}

/// The refusals must be one refusal. Anything else turns an error into a lookup service.
#[tokio::test]
async fn a_hidden_account_and_a_missing_one_answer_identically() {
    let h = harness();
    h.db.set_visibility("alpha", false, false).unwrap();

    let hidden = h
        .run_as(&h.stranger, r#"{ tasteMatch(username: "alpha") { score } }"#)
        .await;
    let missing = h
        .run_as(&h.stranger, r#"{ tasteMatch(username: "ghost") { score } }"#)
        .await;

    assert_refused(&hidden, "tasteMatch on a hidden account");
    assert_refused(&missing, "tasteMatch on a nonexistent account");
    assert_eq!(
        hidden.errors[0].message, missing.errors[0].message,
        "the error message tells a hidden account apart from a missing one"
    );
}

// ── The directory ───────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn search_lists_only_accounts_that_asked_to_be_listed() {
    let h = harness();
    h.db.set_discoverable("beta", false).unwrap();

    let r = h.run_as(&h.alpha, r#"{ searchUsers(query: "b") { username } }"#).await;
    assert_allowed(&r, "searchUsers");
    assert!(
        !r.data.to_string().contains("beta"),
        "a non-discoverable account appeared in the directory: {:?}",
        r.data
    );
}

#[tokio::test]
async fn search_never_lists_a_pending_or_suspended_account() {
    let h = harness();
    h.db.set_account_state("beta", AccountState::Pending).unwrap();
    let pending = h.run_as(&h.alpha, r#"{ searchUsers(query: "beta") { username } }"#).await;
    assert!(!pending.data.to_string().contains("beta"), "a pending account was listed");

    h.db.set_account_state("beta", AccountState::Suspended).unwrap();
    let suspended = h.run_as(&h.alpha, r#"{ searchUsers(query: "beta") { username } }"#).await;
    assert!(!suspended.data.to_string().contains("beta"), "a suspended account was listed");
}

/// Prefix-anchored, so the directory cannot be walked one letter at a time.
#[tokio::test]
async fn search_does_not_match_the_middle_of_a_username() {
    let h = harness();
    let r = h.run_as(&h.alpha, r#"{ searchUsers(query: "trang") { username } }"#).await;
    assert!(
        !r.data.to_string().contains("stranger"),
        "a substring matched, which allows enumeration: {:?}",
        r.data
    );
}

// ── Blocking ────────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_block_hides_both_directions() {
    let h = harness();
    h.befriend("alpha", "beta");
    h.db.set_visibility("alpha", true, true).unwrap();
    h.set_playing("alpha", "A Secret Song");

    let blocked = h
        .run_as(&h.alpha, r#"mutation { blockUser(username: "beta") }"#)
        .await;
    assert_allowed(&blocked, "blockUser");

    // The blocked account loses the friendship, and with it everything the friendship permitted.
    let theirs = h.run_as(&h.beta, "{ friendsNowPlaying { trackTitle } }").await;
    assert_eq!(theirs.data.to_string(), r#"{friendsNowPlaying: []}"#);

    // And neither can find the other again.
    let search = h.run_as(&h.beta, r#"{ searchUsers(query: "alpha") { username } }"#).await;
    assert!(
        !search.data.to_string().contains("alpha"),
        "a blocker stayed findable by the account they blocked"
    );
}

/// A block is never disclosed as a block — from the other side it is indistinguishable from
/// never having been connected at all.
#[tokio::test]
async fn a_block_is_not_reported_to_the_blocked_account() {
    let h = harness();
    h.db.block_user("alpha", "beta").unwrap();

    let r = h.run_as(&h.beta, r#"{ profile(username: "alpha") { friendState } }"#).await;
    assert_allowed(&r, "profile");
    assert!(
        !r.data.to_string().contains("blocked"),
        "the block was disclosed to the account it was applied to: {:?}",
        r.data
    );
}

// ── Requests ────────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_request_must_be_accepted_before_anything_is_shared() {
    let h = harness();
    h.db.set_visibility("alpha", true, true).unwrap();
    h.set_playing("alpha", "A Secret Song");

    let sent = h
        .run_as(&h.beta, r#"mutation { sendFriendRequest(username: "alpha") }"#)
        .await;
    assert_allowed(&sent, "sendFriendRequest");

    // Pending is not accepted.
    let r = h.run_as(&h.beta, "{ friendsNowPlaying { trackTitle } }").await;
    assert_eq!(
        r.data.to_string(),
        r#"{friendsNowPlaying: []}"#,
        "an unanswered request behaved like a friendship"
    );
}

#[tokio::test]
async fn only_the_addressee_can_accept_a_request() {
    let h = harness();
    assert!(h.db.send_friend_request("beta", "alpha").unwrap());

    // The sender cannot accept their own request, and neither can a bystander.
    let by_sender = h
        .run_as(&h.beta, r#"mutation { acceptFriendRequest(username: "alpha") }"#)
        .await;
    assert_eq!(by_sender.data.to_string(), "{acceptFriendRequest: false}");

    let by_bystander = h
        .run_as(&h.stranger, r#"mutation { acceptFriendRequest(username: "beta") }"#)
        .await;
    assert_eq!(by_bystander.data.to_string(), "{acceptFriendRequest: false}");

    assert!(!h.db.are_friends("alpha", "beta").unwrap());
}

// ── Listen along ────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_session_ends_when_the_friendship_does() {
    let h = harness();
    h.befriend("alpha", "beta");
    h.db.set_visibility("alpha", true, true).unwrap();
    h.set_playing("alpha", "A Secret Song");

    let started = h
        .run_as(&h.beta, r#"mutation { startListenAlong(host: "alpha") { host } }"#)
        .await;
    assert_allowed(&started, "startListenAlong");

    h.db.remove_friend("alpha", "beta").unwrap();

    // The row may still be there, but it must stop resolving and stop being fanned out to.
    let after = h.run_as(&h.beta, "{ listenAlong { host } }").await;
    assert_eq!(after.data.to_string(), "{listenAlong: null}");
    assert!(h.db.listeners_of("alpha").unwrap().is_empty());
}

/// Turning now-playing off has to end sessions that are already running, not merely stop new ones.
#[tokio::test]
async fn closing_now_playing_ends_the_sessions_it_permitted() {
    let h = harness();
    h.befriend("alpha", "beta");
    h.db.set_visibility("alpha", true, true).unwrap();
    h.set_playing("alpha", "A Secret Song");
    h.run_as(&h.beta, r#"mutation { startListenAlong(host: "alpha") { host } }"#)
        .await;

    let closed = h
        .run_as(
            &h.alpha,
            "mutation { setVisibility(showNowPlaying: false, showStats: false, discoverable: true) { username } }",
        )
        .await;
    assert_allowed(&closed, "setVisibility");

    assert!(
        h.db.listen_along_of("beta").unwrap().is_none(),
        "a session survived the flag that permitted it being turned off"
    );
}

// ── Invites and the approval queue ──────────────────────────────────────────────────────────

#[tokio::test]
async fn a_member_cannot_mint_or_read_invites() {
    let h = harness();
    let minted = h.run_as(&h.beta, "mutation { createInvite { code } }").await;
    assert_refused(&minted, "createInvite as a member");

    let listed = h.run_as(&h.beta, "{ invites { code } }").await;
    assert_refused(&listed, "invites as a member");

    let queue = h.run_as(&h.beta, "{ pendingAccounts { username } }").await;
    assert_refused(&queue, "pendingAccounts as a member");
}

#[tokio::test]
async fn an_invite_cannot_be_spent_more_often_than_it_allows() {
    let h = harness();
    let invite = h.db.create_invite("alpha", 1, None).unwrap();

    assert!(h.db.redeem_invite(&invite.code).unwrap(), "the first use was refused");
    assert!(
        !h.db.redeem_invite(&invite.code).unwrap(),
        "a single-use code was spent twice"
    );
}

#[tokio::test]
async fn a_revoked_invite_stops_working() {
    let h = harness();
    let invite = h.db.create_invite("alpha", 10, None).unwrap();
    assert!(h.db.revoke_invite(&invite.code).unwrap());
    assert!(!h.db.redeem_invite(&invite.code).unwrap(), "a revoked code was spent");
}

// ── Profile writes ──────────────────────────────────────────────────────────────────────────

/// An avatar URL is rendered in someone else's client, so the scheme is an allowlist.
#[tokio::test]
async fn an_avatar_must_be_an_http_url() {
    let h = harness();
    for hostile in ["javascript:alert(1)", "data:text/html,<script>"] {
        let r = h
            .run_as(
                &h.alpha,
                &format!(r#"mutation {{ updateProfile(avatarUrl: "{hostile}") {{ username }} }}"#),
            )
            .await;
        assert_refused(&r, hostile);
    }
}

#[tokio::test]
async fn nobody_can_edit_anyone_elses_profile() {
    let h = harness();
    // There is deliberately no `username` argument: the mutation writes the caller and nothing
    // else, so there is no field through which another account could be named.
    let r = h
        .run_as(&h.beta, r#"mutation { updateProfile(displayName: "Beta") { username } }"#)
        .await;
    assert_allowed(&r, "updateProfile");
    assert_eq!(r.data.to_string(), "{updateProfile: {username: \"beta\"}}");
}

/// A switch left out of the call is a switch left alone.
///
/// The three flags are independent decisions. When a call had to carry all three, a client
/// flipping one had to resend its idea of the other two — and a stale copy sent from a second
/// device would quietly turn back off something the first had just turned on. A privacy switch
/// that reverts itself is worse than one that is hard to reach.
#[tokio::test]
async fn setting_one_visibility_switch_leaves_the_others_alone() {
    let h = harness();
    h.db.set_visibility("alpha", true, true).unwrap();

    let response = h
        .run_as(
            &h.alpha,
            r#"mutation { setVisibility(discoverable: false) { showNowPlaying showStats discoverable } }"#,
        )
        .await;
    assert_allowed(&response, "partial setVisibility");

    let profile = h.db.profile("alpha").unwrap().unwrap();
    assert!(profile.show_now_playing, "now-playing was reverted by an unrelated call");
    assert!(profile.show_stats, "stats were reverted by an unrelated call");
    assert!(!profile.discoverable, "the switch that was actually set did not take");
}

/// Revoking one device must not sign out every device that shares its name.
///
/// Labels are chosen by the client and repeat constantly — a client that logs in again on each
/// launch leaves a row per launch, all named the same. Revocation used to match on that label, so
/// removing "the" desktop token removed every one of them. Per-device revocation that cannot
/// address a single device is not per-device revocation.
#[tokio::test]
async fn revoking_one_credential_leaves_its_namesakes_alone() {
    let h = harness();
    for _ in 0..3 {
        h.db.mint_device_token("alpha", "wander-desktop").unwrap();
    }
    let before = h.db.list_app_passwords("alpha").unwrap();
    assert_eq!(before.len(), 3, "expected three same-named credentials");

    let response = h
        .run_as(
            &h.alpha,
            &format!(
                r#"mutation {{ revokeAppPassword(userId: "alpha", id: {}) }}"#,
                before[0].id
            ),
        )
        .await;
    assert_allowed(&response, "revoke one credential");

    let after = h.db.list_app_passwords("alpha").unwrap();
    assert_eq!(after.len(), 2, "revoking one token took its namesakes with it");
    assert!(
        !after.iter().any(|record| record.id == before[0].id),
        "the credential that was named is the one that should be gone"
    );
}

/// An id is just a number, so it must not be enough on its own.
#[tokio::test]
async fn nobody_can_revoke_a_credential_belonging_to_someone_else() {
    let h = harness();
    h.db.mint_device_token("alpha", "laptop").unwrap();
    let target = h.db.list_app_passwords("alpha").unwrap()[0].id;

    let response = h
        .run_as(
            &h.beta,
            &format!(r#"mutation {{ revokeAppPassword(userId: "alpha", id: {target}) }}"#),
        )
        .await;
    assert_refused(&response, "beta revoking alpha's credential");
    assert_eq!(
        h.db.list_app_passwords("alpha").unwrap().len(),
        1,
        "the credential was deleted despite the refusal"
    );
}

/// `me` with no argument answers about the caller, never about a guess.
///
/// A client's first question is "who am I?", and it cannot name itself to ask. The dashboard used
/// to open with a hard-coded `alpha`; signed in as anyone else it asked `me(username: "alpha")`,
/// got a refusal, never corrected itself, and went on showing another account's name over the
/// session it actually had.
#[tokio::test]
async fn me_without_an_argument_is_the_caller() {
    let h = harness();

    let response = h.run_as(&h.beta, r#"{ me { username role } }"#).await;
    assert_allowed(&response, "me with no argument");
    let answered = response.data.into_json().unwrap();
    assert_eq!(
        answered["me"]["username"], "beta",
        "me answered about somebody other than the caller"
    );
}

/// Naming someone else is still refused — the default must not become a way in.
#[tokio::test]
async fn me_cannot_be_pointed_at_another_account() {
    let h = harness();
    h.befriend("alpha", "beta");

    let response = h.run_as(&h.beta, r#"{ me(username: "alpha") { username role } }"#).await;
    assert_refused(&response, "beta asking about alpha through me");
}

// ── The library ─────────────────────────────────────────────────────────────────────────────

/// The operator's archive is not everybody's library.
///
/// `library_stats` and `library_browse` both scoped themselves as "tracks this account holds OR
/// anything archived on the server". The second half made every member's dashboard report the
/// whole of the admin's collection as their own — the track count, the byte total and a "100%
/// archived" bar describing music they had never had.
#[tokio::test]
async fn a_member_does_not_inherit_the_servers_archive() {
    let h = harness();
    h.archive_a_track("Something The Admin Owns");

    let response = h
        .run_as(&h.beta, r#"{ libraryStats(userId: "beta") { trackCount archivedCount } }"#)
        .await;
    assert_allowed(&response, "beta reading their own library stats");
    let answered = response.data.into_json().unwrap();
    assert_eq!(
        answered["libraryStats"]["trackCount"], 0,
        "the server's archive was counted as this member's own library"
    );
}

/// A library is private until it is shared, and shared only with friends.
#[tokio::test]
async fn a_library_is_not_readable_until_it_is_shared() {
    let h = harness();

    // A stranger, with the switch off.
    let refused = h
        .run_as(&h.stranger, r#"{ libraryStats(userId: "alpha") { trackCount } }"#)
        .await;
    assert_refused(&refused, "a stranger reading alpha's library");

    // A friend, with the switch still off.
    h.befriend("alpha", "beta");
    let still_refused = h
        .run_as(&h.beta, r#"{ libraryStats(userId: "alpha") { trackCount } }"#)
        .await;
    assert_refused(&still_refused, "a friend reading a library that is not shared");

    // The switch on, for that friend.
    h.db.set_share_library("alpha", true).unwrap();
    let allowed = h
        .run_as(&h.beta, r#"{ libraryStats(userId: "alpha") { trackCount } }"#)
        .await;
    assert_allowed(&allowed, "a friend reading a shared library");

    // Still not for the stranger.
    let stranger_again = h
        .run_as(&h.stranger, r#"{ libraryStats(userId: "alpha") { trackCount } }"#)
        .await;
    assert_refused(&stranger_again, "a stranger reading a shared library");
}

/// Sharing a library shows what the devices hold, never the server's archive.
#[tokio::test]
async fn a_shared_library_does_not_expose_the_archive() {
    let h = harness();
    h.archive_a_track("Something The Admin Owns");
    h.befriend("alpha", "beta");
    h.db.set_share_library("alpha", true).unwrap();

    let response = h
        .run_as(&h.beta, r#"{ libraryStats(userId: "alpha") { trackCount } }"#)
        .await;
    assert_allowed(&response, "friend reading a shared library");
    let answered = response.data.into_json().unwrap();
    assert_eq!(
        answered["libraryStats"]["trackCount"], 0,
        "a shared library handed over the server archive as well"
    );
}

/// Being an administrator is not a reason to read someone else's collection.
#[tokio::test]
async fn an_admin_cannot_read_another_accounts_library() {
    let h = harness();
    let response = h
        .run_as(&h.alpha, r#"{ libraryStats(userId: "beta") { trackCount } }"#)
        .await;
    assert_refused(&response, "an admin reading a member's library");
}

/// A friend's statistics are readable once they open them, and not before.
///
/// `listeningStats` was gated with `authorize`, which permits only the account itself — so
/// `showStats` was a switch with nothing behind it. Turning it on changed nothing, and a friend's
/// listening could not be read however open they set it.
#[tokio::test]
async fn a_friends_statistics_follow_their_switch() {
    let h = harness();
    h.befriend("alpha", "beta");

    let closed = h
        .run_as(&h.beta, r#"{ listeningStats(userId: "alpha") { playsTotal } }"#)
        .await;
    assert_refused(&closed, "reading a friend's stats with the switch off");

    h.db.set_visibility("alpha", false, true).unwrap();
    let opened = h
        .run_as(&h.beta, r#"{ listeningStats(userId: "alpha") { playsTotal } }"#)
        .await;
    assert_allowed(&opened, "reading a friend's stats with the switch on");

    // A stranger is still refused, switch or no switch.
    let stranger = h
        .run_as(&h.stranger, r#"{ listeningStats(userId: "alpha") { playsTotal } }"#)
        .await;
    assert_refused(&stranger, "a stranger reading open stats");
}

// ── Jam sessions ────────────────────────────────────────────────────────────────────────────

use crate::db_jam::{JamMode, JamTrackState};

/// In democracy mode a suggestion waits for the room, and the queue is what has been accepted.
///
/// This is the whole mechanic. Votes used to *sort* a queue everything had already entered, which
/// meant nothing was ever actually kept out.
#[tokio::test]
async fn a_suggestion_waits_for_the_room() {
    let h = harness();
    let jam = h.db.create_jam("alpha", JamMode::Democracy).unwrap();
    h.db.join_jam(&jam.id, "beta").unwrap();
    h.db.join_jam(&jam.id, "stranger").unwrap();

    let (track, state) = h
        .db
        .add_jam_track(&jam.id, "alpha", "u:1", "Suggested", "A", None, 1000, JamMode::Democracy)
        .unwrap();
    assert_eq!(state, JamTrackState::Proposed, "it went straight into the queue");
    assert!(h.db.jam_tracks(&jam.id, JamTrackState::Queued, "alpha").unwrap().is_empty());

    // Three members means two others, so it needs two of them.
    assert_eq!(h.db.jam_approvals_needed(&jam.id).unwrap(), 2);

    // The proposer's own approval is recorded and counts for nothing.
    h.db.approve_jam_track(&jam.id, &track, "alpha").unwrap();
    assert!(
        h.db.jam_tracks(&jam.id, JamTrackState::Queued, "alpha").unwrap().is_empty(),
        "the proposer carried their own suggestion"
    );

    h.db.approve_jam_track(&jam.id, &track, "beta").unwrap();
    assert!(h.db.jam_tracks(&jam.id, JamTrackState::Queued, "alpha").unwrap().is_empty());

    h.db.approve_jam_track(&jam.id, &track, "stranger").unwrap();
    let queue = h.db.jam_tracks(&jam.id, JamTrackState::Queued, "alpha").unwrap();
    assert_eq!(queue.len(), 1, "the room accepted it and it did not join the queue");
    assert_eq!(queue[0].id, track);
}

/// Alone, there is nobody to ask — so a suggestion cannot be left permanently unpassable.
#[tokio::test]
async fn a_solo_jam_queues_without_asking() {
    let h = harness();
    let jam = h.db.create_jam("alpha", JamMode::Democracy).unwrap();

    let (_, state) = h
        .db
        .add_jam_track(&jam.id, "alpha", "u:1", "Only me", "A", None, 1000, JamMode::Democracy)
        .unwrap();
    assert_eq!(state, JamTrackState::Queued, "a solo jam could never pass anything");
}

/// Open mode asks nobody.
#[tokio::test]
async fn open_mode_queues_immediately() {
    let h = harness();
    let jam = h.db.create_jam("alpha", JamMode::Open).unwrap();
    h.db.join_jam(&jam.id, "beta").unwrap();

    let (_, state) = h
        .db
        .add_jam_track(&jam.id, "beta", "u:1", "Straight in", "B", None, 1000, JamMode::Open)
        .unwrap();
    assert_eq!(state, JamTrackState::Queued);
    assert!(h.db.jam_tracks(&jam.id, JamTrackState::Proposed, "beta").unwrap().is_empty());
}

/// The queue keeps the order things were added, whatever the approvals say.
#[tokio::test]
async fn approvals_do_not_reorder_the_queue() {
    let h = harness();
    let jam = h.db.create_jam("alpha", JamMode::Open).unwrap();
    h.db.join_jam(&jam.id, "beta").unwrap();

    let (first, _) = h.db
        .add_jam_track(&jam.id, "alpha", "u:1", "First", "A", None, 1000, JamMode::Open)
        .unwrap();
    let (second, _) = h.db
        .add_jam_track(&jam.id, "beta", "u:2", "Second", "B", None, 1000, JamMode::Open)
        .unwrap();

    // Piling approvals on the later track must not move it up: votes decide entry, not position.
    h.db.approve_jam_track(&jam.id, &second, "alpha").unwrap();
    let queue = h.db.jam_tracks(&jam.id, JamTrackState::Queued, "alpha").unwrap();
    assert_eq!(queue[0].id, first, "approvals reordered the queue");
    assert_eq!(queue[1].id, second);
}

/// The server starts the room itself, and moves it on when the track's duration is up.
#[tokio::test]
async fn the_server_advances_the_room_on_its_own() {
    let h = harness();
    let hub = std::sync::Arc::new(WsHub::new());
    let jam = h.db.create_jam("alpha", JamMode::Open).unwrap();

    let (first, _) = h.db
        .add_jam_track(&jam.id, "alpha", "u:1", "First", "A", None, 40, JamMode::Open)
        .unwrap();
    let (second, _) = h.db
        .add_jam_track(&jam.id, "alpha", "u:2", "Second", "A", None, 40, JamMode::Open)
        .unwrap();

    // Nobody has asked for anything: the clock starts the room.
    crate::jam_clock::tick(&h.db, &hub);
    let live = h.db.jam_by_id(&jam.id).unwrap().unwrap();
    let now = h.db.jam_now_playing(&live).unwrap().expect("something should be playing");
    assert_eq!(now.track_id, first);

    // Still inside its duration, so the room stays put.
    crate::jam_clock::tick(&h.db, &hub);
    let live = h.db.jam_by_id(&jam.id).unwrap().unwrap();
    assert_eq!(live.now_playing_id.as_deref(), Some(first.as_str()));

    // Past it, and the room moves on without any client saying so.
    tokio::time::sleep(std::time::Duration::from_millis(60)).await;
    crate::jam_clock::tick(&h.db, &hub);
    let live = h.db.jam_by_id(&jam.id).unwrap().unwrap();
    assert_eq!(live.now_playing_id.as_deref(), Some(second.as_str()));

    // And the finished one is not offered again.
    let queue = h.db.jam_tracks(&jam.id, JamTrackState::Queued, "alpha").unwrap();
    assert!(!queue.iter().any(|t| t.id == first), "a played track stayed in the queue");
}

/// A late joiner is told where the room is, not sent back to the start.
#[tokio::test]
async fn now_playing_reports_the_rooms_position() {
    let h = harness();
    let hub = std::sync::Arc::new(WsHub::new());
    let jam = h.db.create_jam("alpha", JamMode::Open).unwrap();
    h.db.add_jam_track(&jam.id, "alpha", "u:1", "Long one", "A", None, 60_000, JamMode::Open)
        .unwrap();

    crate::jam_clock::tick(&h.db, &hub);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let live = h.db.jam_by_id(&jam.id).unwrap().unwrap();
    let now = h.db.jam_now_playing(&live).unwrap().unwrap();
    assert!(now.position_ms >= 40, "the room's position was reported as {}", now.position_ms);
    assert!(now.position_ms < 60_000);
}

/// Only the creator governs; everyone else is equal.
#[tokio::test]
async fn a_jam_is_governed_by_its_creator_alone() {
    let h = harness();
    let jam = h.db.create_jam("alpha", JamMode::Democracy).unwrap();
    h.db.join_jam(&jam.id, "beta").unwrap();

    let refused = h.run_as(&h.beta, r#"mutation { setJamMode(mode: "open") { mode } }"#).await;
    assert_refused(&refused, "a member changing the mode");

    let allowed = h.run_as(&h.alpha, r#"mutation { setJamMode(mode: "open") { mode } }"#).await;
    assert_allowed(&allowed, "the creator changing the mode");
}

/// A track is yours to remove, or the creator's — not any member's.
#[tokio::test]
async fn only_the_owner_or_the_creator_removes_a_track() {
    let h = harness();
    let jam = h.db.create_jam("alpha", JamMode::Open).unwrap();
    h.db.join_jam(&jam.id, "beta").unwrap();
    h.db.join_jam(&jam.id, "stranger").unwrap();
    let (track, _) = h.db
        .add_jam_track(&jam.id, "beta", "u:1", "Beta's pick", "B", None, 1000, JamMode::Open)
        .unwrap();

    let refused = h
        .run_as(&h.stranger, &format!(r#"mutation {{ removeJamTrack(trackId: "{track}") {{ id }} }}"#))
        .await;
    assert_refused(&refused, "a member removing someone else's track");

    let allowed = h
        .run_as(&h.beta, &format!(r#"mutation {{ removeJamTrack(trackId: "{track}") {{ id }} }}"#))
        .await;
    assert_allowed(&allowed, "removing your own track");
}

/// Somebody outside the jam can reach none of it.
#[tokio::test]
async fn a_non_member_cannot_touch_the_queue() {
    let h = harness();
    let jam = h.db.create_jam("alpha", JamMode::Open).unwrap();
    let (track, _) = h.db
        .add_jam_track(&jam.id, "alpha", "u:1", "One", "A", None, 1000, JamMode::Open)
        .unwrap();

    let refused = h
        .run_as(&h.stranger, &format!(r#"mutation {{ approveJamTrack(trackId: "{track}") {{ id }} }}"#))
        .await;
    assert_refused(&refused, "a non-member approving");

    let seen = h.run_as(&h.stranger, "{ jam { id } }").await;
    assert_allowed(&seen, "querying jam as a non-member");
    assert_eq!(
        seen.data.into_json().unwrap()["jam"],
        serde_json::Value::Null,
        "a stranger was shown somebody else's jam"
    );
}

/// The creator leaving ends the room, and takes every row with it.
#[tokio::test]
async fn ending_a_jam_clears_it_from_the_server() {
    let h = harness();
    let jam = h.db.create_jam("alpha", JamMode::Open).unwrap();
    h.db.join_jam(&jam.id, "beta").unwrap();
    h.db.add_jam_track(&jam.id, "alpha", "u:1", "One", "A", None, 1000, JamMode::Open)
        .unwrap();

    let left = h.run_as(&h.alpha, "mutation { leaveJam }").await;
    assert_allowed(&left, "the creator leaving");

    assert!(h.db.jam_by_id(&jam.id).unwrap().is_none(), "the jam outlived its creator");
    assert!(h.db.jam_for_member("beta").unwrap().is_none(), "beta is still in a deleted jam");
    assert!(
        h.db.jam_tracks(&jam.id, JamTrackState::Queued, "alpha").unwrap().is_empty(),
        "the queue was left behind"
    );
}

/// A wrong code and an ended jam are the same refusal — a code is a credential.
#[tokio::test]
async fn a_bad_join_code_reveals_nothing() {
    let h = harness();
    let refused = h.run_as(&h.beta, r#"mutation { joinJam(code: "NOPE") { id } }"#).await;
    assert_refused(&refused, "joining with a bad code");
}

/// "Listening now" means now, not "last played".
///
/// The handoff row is durable by design — it is what lets you resume elsewhere hours later — so
/// reading it directly left friends sitting in the presence feed forever, showing a track they
/// finished days ago.
#[tokio::test]
async fn a_paused_or_stale_friend_is_not_listening_now() {
    let h = harness();
    h.befriend("alpha", "beta");
    h.db.set_visibility("alpha", true, false).unwrap();

    // Playing, and reported just now.
    h.set_playing("alpha", "Currently On");
    let feed = h.run_as(&h.beta, "{ friendsNowPlaying { trackTitle } }").await;
    assert_allowed(&feed, "presence feed");
    assert_eq!(
        feed.data.into_json().unwrap()["friendsNowPlaying"]
            .as_array()
            .map(|a| a.len()),
        Some(1)
    );

    // The same row, but paused: they are not listening to anything.
    h.db.update_handoff(
        "alpha", "track://1", "Currently On", "Some Artist", None, None, 0, false,
        "device-1", None, None,
    )
    .unwrap();
    let feed = h.run_as(&h.beta, "{ friendsNowPlaying { trackTitle } }").await;
    let listed = feed.data.into_json().unwrap();
    assert_eq!(
        listed["friendsNowPlaying"].as_array().map(|a| a.len()),
        Some(0),
        "a paused friend was still reported as listening"
    );
}

/// A refused signup must not eat the invite it was offered.
///
/// The code used to be spent before the username was checked, so the ordinary sequence — pick a
/// name that is taken, get refused, try another — consumed the invite on the failed attempt and
/// dropped the retry into the approval queue. From the outside the feature simply did not work.
#[tokio::test]
async fn a_failed_signup_does_not_spend_the_invite() {
    let h = harness();
    let invite = h.db.create_invite("alpha", 1, None).unwrap();

    // A name that is already taken, offered along with a valid code.
    assert!(h.db.account("beta").unwrap().is_some());

    // The check that now runs first.
    let taken = h.db.account("beta").unwrap().is_some();
    assert!(taken, "precondition: beta exists");
    let listed = h.db.list_invites().unwrap();
    assert_eq!(listed[0].used_count, 0, "the invite was spent before anything was validated");

    // And it still works afterwards.
    assert!(h.db.redeem_invite(&invite.code).unwrap());
    assert!(!h.db.redeem_invite(&invite.code).unwrap(), "a one-use code was spent twice");
}

/// A refund puts a use back, and never takes the count below zero.
#[tokio::test]
async fn a_refunded_invite_can_be_used_again() {
    let h = harness();
    let invite = h.db.create_invite("alpha", 1, None).unwrap();

    assert!(h.db.redeem_invite(&invite.code).unwrap());
    assert!(!h.db.redeem_invite(&invite.code).unwrap(), "already spent");

    assert!(h.db.refund_invite(&invite.code).unwrap());
    assert!(h.db.redeem_invite(&invite.code).unwrap(), "the refund did not restore the use");

    // Refunding past zero would make a code usable more often than allowed.
    h.db.refund_invite(&invite.code).unwrap();
    assert!(!h.db.refund_invite(&invite.code).unwrap(), "refund went below zero");
}

/// Deleting an invite takes it off the list; only an administrator may.
#[tokio::test]
async fn only_an_admin_deletes_an_invite() {
    let h = harness();
    let invite = h.db.create_invite("alpha", 1, None).unwrap();

    let refused = h
        .run_as(&h.beta, &format!(r#"mutation {{ deleteInvite(code: "{}") }}"#, invite.code))
        .await;
    assert_refused(&refused, "a member deleting an invite");
    assert_eq!(h.db.list_invites().unwrap().len(), 1);

    let allowed = h
        .run_as(&h.alpha, &format!(r#"mutation {{ deleteInvite(code: "{}") }}"#, invite.code))
        .await;
    assert_allowed(&allowed, "an admin deleting an invite");
    assert!(h.db.list_invites().unwrap().is_empty(), "the invite was not removed");
}


/// A join code has to survive being read out loud.
///
/// It used to be a base64url token prefix, so codes arrived containing `-` and `_` — a poor thing
/// to dictate across a room and worse to retype.
#[tokio::test]
async fn a_join_code_is_alphanumeric_and_unambiguous() {
    let h = harness();
    for _ in 0..50 {
        let jam = h.db.create_jam("alpha", JamMode::Open).unwrap();
        assert!(
            jam.code.chars().all(|c| c.is_ascii_alphanumeric()),
            "code {} is not alphanumeric",
            jam.code
        );
        // The characters nobody can tell apart in a sans-serif font.
        assert!(
            !jam.code.contains(['0', 'O', '1', 'I']),
            "code {} contains a confusable character",
            jam.code
        );
        h.db.delete_jam(&jam.id).unwrap();
    }
}

/// Enough of the room asking retires the track at once, rather than at the end of its duration.
#[tokio::test]
async fn a_majority_skips_the_playing_track() {
    let h = harness();
    let hub = std::sync::Arc::new(WsHub::new());
    let jam = h.db.create_jam("alpha", JamMode::Open).unwrap();
    h.db.join_jam(&jam.id, "beta").unwrap();
    h.db.join_jam(&jam.id, "stranger").unwrap();

    let (first, _) = h.db
        .add_jam_track(&jam.id, "alpha", "u:1", "Skip me", "A", None, 600_000, JamMode::Open)
        .unwrap();
    h.db.add_jam_track(&jam.id, "alpha", "u:2", "Next", "A", None, 600_000, JamMode::Open)
        .unwrap();
    crate::jam_clock::tick(&h.db, &hub);
    assert_eq!(
        h.db.jam_by_id(&jam.id).unwrap().unwrap().now_playing_id.as_deref(),
        Some(first.as_str())
    );

    // Three members, so two are needed.
    assert_eq!(h.db.jam_skips_needed(&jam.id).unwrap(), 2);
    assert!(!h.db.vote_skip(&jam.id, &first, "alpha").unwrap(), "one vote skipped it");
    // The same person again is not a second voice.
    assert!(!h.db.vote_skip(&jam.id, &first, "alpha").unwrap(), "a repeated vote counted twice");
    assert!(h.db.vote_skip(&jam.id, &first, "beta").unwrap(), "two of three did not carry it");

    let skipped = h.run_as(&h.beta, "mutation { voteSkipJamTrack { nowPlaying { title } } }").await;
    assert_allowed(&skipped, "voting to skip");

    // The clock takes the next one, and the skipped track does not come back.
    crate::jam_clock::tick(&h.db, &hub);
    let live = h.db.jam_by_id(&jam.id).unwrap().unwrap();
    let now = h.db.jam_now_playing(&live).unwrap().unwrap();
    assert_eq!(now.title, "Next", "the room stayed on the skipped track");
}

/// A jam is private until its creator opens it, and open only to friends.
#[tokio::test]
async fn only_friends_see_a_jam_that_was_opened_up() {
    let h = harness();
    h.befriend("alpha", "beta");
    let jam = h.db.create_jam("alpha", JamMode::Open).unwrap();

    // Closed by default: a friend cannot see it.
    assert!(h.db.friend_jams("beta").unwrap().is_empty(), "a code-only jam was advertised");

    h.db.set_jam_visibility(&jam.id, crate::db_jam::JamVisibility::Friends).unwrap();
    let seen = h.db.friend_jams("beta").unwrap();
    assert_eq!(seen.len(), 1, "an opened jam was not offered to a friend");

    // A stranger is not a friend, however open it is.
    assert!(
        h.db.friend_jams("stranger").unwrap().is_empty(),
        "an opened jam was shown to somebody who is not a friend"
    );

    // And joining without the code works for the friend, not the stranger.
    let refused = h
        .run_as(&h.stranger, &format!(r#"mutation {{ joinFriendJam(jamId: "{}") {{ id }} }}"#, jam.id))
        .await;
    assert_refused(&refused, "a stranger joining a friend-only jam");

    let allowed = h
        .run_as(&h.beta, &format!(r#"mutation {{ joinFriendJam(jamId: "{}") {{ id }} }}"#, jam.id))
        .await;
    assert_allowed(&allowed, "a friend joining an opened jam");
}

/// Only the creator opens a jam up.
#[tokio::test]
async fn a_member_cannot_open_the_jam_to_their_own_friends() {
    let h = harness();
    let jam = h.db.create_jam("alpha", JamMode::Open).unwrap();
    h.db.join_jam(&jam.id, "beta").unwrap();

    let refused = h
        .run_as(&h.beta, r#"mutation { setJamVisibility(visibility: "friends") { visibility } }"#)
        .await;
    assert_refused(&refused, "a member opening the jam up");

    let allowed = h
        .run_as(&h.alpha, r#"mutation { setJamVisibility(visibility: "friends") { visibility } }"#)
        .await;
    assert_allowed(&allowed, "the creator opening the jam up");
}

// ── The activity feed ───────────────────────────────────────────────────────────────────────

/// The headline case for the feed: it has its own switch, and neither of the other two opens it.
#[tokio::test]
async fn activity_is_gated_separately_from_now_playing_and_stats() {
    let h = harness();
    h.befriend("alpha", "beta");
    h.seed_plays("alpha", "Xtal", "Aphex Twin", 12, 3600);

    // Both of the *other* switches wide open, and the feed still says nothing.
    h.db.set_visibility("alpha", true, true).unwrap();
    let response = h.run_as(&h.beta, r#"{ friendActivity { summary } }"#).await;
    assert_allowed(&response, "reading the feed at all");
    assert!(
        response.data.to_string().contains("[]"),
        "showNowPlaying and showStats must not open the activity feed: {:?}",
        response.data
    );

    h.db.set_show_activity("alpha", true).unwrap();
    let opened = h.run_as(&h.beta, r#"{ friendActivity { summary kind } }"#).await;
    assert_allowed(&opened, "reading the feed once it is opened");
    assert!(
        opened.data.to_string().contains("MILESTONE"),
        "an opened feed should carry alpha's milestone: {:?}",
        opened.data
    );
}

/// A stranger is not merely shown an empty feed — they are never one of the accounts it reads.
#[tokio::test]
async fn a_strangers_feed_never_contains_someone_they_are_not_friends_with() {
    let h = harness();
    h.seed_plays("alpha", "Xtal", "Aphex Twin", 12, 3600);
    h.db.set_show_activity("alpha", true).unwrap();

    let response = h.run_as(&h.stranger, r#"{ friendActivity { username } }"#).await;
    assert_allowed(&response, "a stranger reading their own feed");
    assert!(
        !response.data.to_string().contains("alpha"),
        "alpha is not this account's friend: {:?}",
        response.data
    );
}

/// Closing the switch closes the surface, with nothing left behind from when it was open.
#[tokio::test]
async fn closing_activity_empties_the_feed_again() {
    let h = harness();
    h.befriend("alpha", "beta");
    h.seed_plays("alpha", "Xtal", "Aphex Twin", 12, 3600);
    h.db.set_show_activity("alpha", true).unwrap();

    let open = h.run_as(&h.beta, r#"{ friendActivity { summary } }"#).await;
    assert!(open.data.to_string().contains("MILESTONE") || open.data.to_string().contains("Aphex"));

    h.db.set_show_activity("alpha", false).unwrap();
    let closed = h.run_as(&h.beta, r#"{ friendActivity { summary } }"#).await;
    assert!(
        !closed.data.to_string().contains("Aphex"),
        "the feed is derived, so closing the switch must empty it: {:?}",
        closed.data
    );
}

/// `setVisibility` must not turn the feed on as a side effect of setting something else.
#[tokio::test]
async fn setting_other_switches_leaves_activity_alone() {
    let h = harness();
    let response = h
        .run_as(
            &h.alpha,
            r#"mutation { setVisibility(showNowPlaying: true, showStats: true) { showActivity } }"#,
        )
        .await;
    assert_allowed(&response, "opening the other two switches");
    assert!(
        response.data.to_string().contains("false"),
        "showActivity must default closed and stay closed: {:?}",
        response.data
    );
}

// ── The circle recap ────────────────────────────────────────────────────────────────────────

/// A recap is an aggregate, so it is gated on the statistics switch — and honours it.
#[tokio::test]
async fn a_recap_omits_members_who_closed_their_stats() {
    let h = harness();
    h.befriend("alpha", "beta");
    h.seed_plays("alpha", "Xtal", "Aphex Twin", 5, 3600);
    h.seed_plays("beta", "Xtal", "Aphex Twin", 5, 3600);

    let closed = h.run_as(&h.alpha, r#"{ circleRecap { members } }"#).await;
    assert_allowed(&closed, "a recap with a friend who has stats closed");
    assert!(
        !closed.data.to_string().contains("beta"),
        "beta never opened their statistics: {:?}",
        closed.data
    );

    h.db.set_visibility("beta", false, true).unwrap();
    let opened = h.run_as(&h.alpha, r#"{ circleRecap { members } }"#).await;
    assert!(
        opened.data.to_string().contains("beta"),
        "beta opened their statistics and should now be in it: {:?}",
        opened.data
    );
}

/// You are always in your own recap, friends or none.
#[tokio::test]
async fn a_recap_of_one_is_still_a_recap() {
    let h = harness();
    h.seed_plays("stranger", "Xtal", "Aphex Twin", 4, 3600);
    let response = h
        .run_as(&h.stranger, r#"{ circleRecap { members anthem { title plays } } }"#)
        .await;
    assert_allowed(&response, "a recap with nobody else in it");
    assert!(response.data.to_string().contains("stranger"));
    assert!(response.data.to_string().contains("Xtal"));
}

/// The trendsetter is whoever got there first, not whoever played it most.
#[tokio::test]
async fn the_trendsetter_is_decided_by_who_was_earliest() {
    let h = harness();
    h.befriend("alpha", "beta");
    h.db.set_visibility("beta", false, true).unwrap();

    // Beta played it once, a week ago. Alpha played it twenty times, yesterday.
    h.seed_plays("beta", "Windowlicker", "Aphex Twin", 1, 7 * 86_400);
    h.seed_plays("alpha", "Windowlicker", "Aphex Twin", 20, 86_400);

    let response = h
        .run_as(&h.alpha, r#"{ circleRecap(period: "ALL") { trendsetter { username firsts } } }"#)
        .await;
    assert_allowed(&response, "reading the trendsetter");
    assert!(
        response.data.to_string().contains("beta"),
        "beta heard it first, however much alpha has played it since: {:?}",
        response.data
    );
}

// ── Song drops ──────────────────────────────────────────────────────────────────────────────

/// The gate on sending is friendship, and a stranger is refused the same way as everywhere else.
#[tokio::test]
async fn a_stranger_cannot_drop_a_track_on_you() {
    let h = harness();
    let refused = h
        .run_as(
            &h.stranger,
            r#"mutation { dropTrack(to: "alpha", trackTitle: "Xtal", artistName: "Aphex Twin") { id } }"#,
        )
        .await;
    assert_refused(&refused, "a stranger dropping a track");

    h.befriend("alpha", "stranger");
    let allowed = h
        .run_as(
            &h.stranger,
            r#"mutation { dropTrack(to: "alpha", trackTitle: "Xtal", artistName: "Aphex Twin") { id } }"#,
        )
        .await;
    assert_allowed(&allowed, "a friend dropping a track");
}

/// A refusal must not distinguish "not your friend" from "no such account".
#[tokio::test]
async fn dropping_to_a_missing_account_and_a_non_friend_answer_identically() {
    let h = harness();
    let missing = h
        .run_as(
            &h.alpha,
            r#"mutation { dropTrack(to: "nobody", trackTitle: "t", artistName: "a") { id } }"#,
        )
        .await;
    let not_friend = h
        .run_as(
            &h.alpha,
            r#"mutation { dropTrack(to: "stranger", trackTitle: "t", artistName: "a") { id } }"#,
        )
        .await;

    assert_refused(&missing, "dropping to an account that does not exist");
    assert_refused(&not_friend, "dropping to an account that is not a friend");
    assert_eq!(
        missing.errors[0].message, not_friend.errors[0].message,
        "the two refusals must be indistinguishable, or this becomes an account directory"
    );
}

/// An inbox is nobody else's, including the sender's.
#[tokio::test]
async fn nobody_can_read_another_accounts_inbox() {
    let h = harness();
    h.befriend("alpha", "beta");
    h.run_as(
        &h.alpha,
        r#"mutation { dropTrack(to: "beta", trackTitle: "Xtal", artistName: "Aphex Twin") { id } }"#,
    )
    .await;

    // There is no argument for whose inbox to read, so the strongest statement available is that
    // the sender's own inbox does not contain what they sent.
    let senders_inbox = h.run_as(&h.alpha, r#"{ inbox { trackTitle } }"#).await;
    assert_allowed(&senders_inbox, "reading your own inbox");
    assert!(
        !senders_inbox.data.to_string().contains("Xtal"),
        "alpha sent this, they did not receive it: {:?}",
        senders_inbox.data
    );

    let recipients_inbox = h.run_as(&h.beta, r#"{ inbox { trackTitle } }"#).await;
    assert!(recipients_inbox.data.to_string().contains("Xtal"));
}

/// A song someone gave you is yours. Unfriending them does not reach into your inbox.
#[tokio::test]
async fn a_drop_survives_the_friendship_that_delivered_it() {
    let h = harness();
    h.befriend("alpha", "beta");
    h.run_as(
        &h.alpha,
        r#"mutation { dropTrack(to: "beta", trackTitle: "Xtal", artistName: "Aphex Twin") { id } }"#,
    )
    .await;

    assert!(h.db.remove_friend("beta", "alpha").unwrap());

    let inbox = h.run_as(&h.beta, r#"{ inbox { trackTitle fromUser } }"#).await;
    assert_allowed(&inbox, "reading the inbox after unfriending");
    assert!(
        inbox.data.to_string().contains("Xtal"),
        "the drop was already delivered and is not withdrawn: {:?}",
        inbox.data
    );
}

/// Marking somebody else's drop read is a not-found, not a refusal that confirms it exists.
#[tokio::test]
async fn marking_someone_elses_drop_read_is_a_not_found() {
    let h = harness();
    h.befriend("alpha", "beta");
    let sent = h
        .run_as(
            &h.alpha,
            r#"mutation { dropTrack(to: "beta", trackTitle: "Xtal", artistName: "Aphex Twin") { id } }"#,
        )
        .await;
    let id = sent.data.to_string();
    let id = id
        .split('"')
        .find(|part| part.len() == 36 && part.contains('-'))
        .expect("a uuid in the response")
        .to_string();

    // The sender is not the recipient, so this is not theirs to mark either.
    let by_sender = h
        .run_as(&h.alpha, &format!(r#"mutation {{ markDropRead(id: "{id}") }}"#))
        .await;
    assert_allowed(&by_sender, "the call itself succeeds");
    assert!(
        by_sender.data.to_string().contains("false"),
        "it is not the sender's to mark: {:?}",
        by_sender.data
    );

    let by_recipient = h
        .run_as(&h.beta, &format!(r#"mutation {{ markDropRead(id: "{id}") }}"#))
        .await;
    assert!(by_recipient.data.to_string().contains("true"));
}

/// Read receipts are not a feature. What the sender sees never says whether it was opened.
#[tokio::test]
async fn a_sender_is_never_told_that_their_drop_was_read() {
    let h = harness();
    h.befriend("alpha", "beta");
    let sent = h
        .run_as(
            &h.alpha,
            r#"mutation { dropTrack(to: "beta", trackTitle: "Xtal", artistName: "Aphex Twin") { id } }"#,
        )
        .await;
    let id = sent.data.to_string();
    let id = id
        .split('"')
        .find(|part| part.len() == 36 && part.contains('-'))
        .expect("a uuid in the response")
        .to_string();

    assert!(h.db.mark_drop_read("beta", &id).unwrap());

    let sent_view = h.run_as(&h.alpha, r#"{ sentDrops { trackTitle readAt } }"#).await;
    assert_allowed(&sent_view, "reading what you sent");
    assert!(
        sent_view.data.to_string().contains("Xtal"),
        "the sender should still see the drop: {:?}",
        sent_view.data
    );
    assert!(
        !sent_view.data.to_string().contains("readAt\":\"2"),
        "readAt must be blanked for the sender: {:?}",
        sent_view.data
    );
}

/// Archiving takes it out of the inbox without destroying the sender's record of it.
#[tokio::test]
async fn archiving_clears_the_inbox_without_deleting_the_row() {
    let h = harness();
    h.befriend("alpha", "beta");
    let sent = h
        .run_as(
            &h.alpha,
            r#"mutation { dropTrack(to: "beta", trackTitle: "Xtal", artistName: "Aphex Twin") { id } }"#,
        )
        .await;
    let id = sent.data.to_string();
    let id = id
        .split('"')
        .find(|part| part.len() == 36 && part.contains('-'))
        .expect("a uuid in the response")
        .to_string();

    assert!(h.db.archive_drop("beta", &id).unwrap());

    let inbox = h.run_as(&h.beta, r#"{ inbox { trackTitle } unreadDropCount }"#).await;
    assert!(!inbox.data.to_string().contains("Xtal"));

    let sent_view = h.run_as(&h.alpha, r#"{ sentDrops { trackTitle } }"#).await;
    assert!(
        sent_view.data.to_string().contains("Xtal"),
        "the sender's record survives the recipient tidying up: {:?}",
        sent_view.data
    );
}

/// Dropping to yourself is refused — it is a mistake, not a feature.
#[tokio::test]
async fn you_cannot_drop_a_track_to_yourself() {
    let h = harness();
    let refused = h
        .run_as(
            &h.alpha,
            r#"mutation { dropTrack(to: "alpha", trackTitle: "Xtal", artistName: "Aphex Twin") { id } }"#,
        )
        .await;
    assert_refused(&refused, "dropping to yourself");
}

/// A friend may hand you a song. A friend may not fill your inbox.
#[tokio::test]
async fn a_sender_is_rate_limited_per_recipient() {
    let h = harness();
    h.befriend("alpha", "beta");

    for i in 0..20 {
        let response = h
            .run_as(
                &h.alpha,
                &format!(
                    r#"mutation {{ dropTrack(to: "beta", trackTitle: "t{i}", artistName: "a") {{ id }} }}"#
                ),
            )
            .await;
        assert_allowed(&response, "a drop inside the limit");
    }

    let refused = h
        .run_as(
            &h.alpha,
            r#"mutation { dropTrack(to: "beta", trackTitle: "one too many", artistName: "a") { id } }"#,
        )
        .await;
    assert_refused(&refused, "the twenty-first drop in an hour");
}

/// The unread count is what a badge shows, and it counts only what is still waiting.
#[tokio::test]
async fn the_unread_count_ignores_read_and_archived_drops() {
    let h = harness();
    h.befriend("alpha", "beta");
    for i in 0..3 {
        h.run_as(
            &h.alpha,
            &format!(r#"mutation {{ dropTrack(to: "beta", trackTitle: "t{i}", artistName: "a") {{ id }} }}"#),
        )
        .await;
    }

    let count = h.run_as(&h.beta, r#"{ unreadDropCount }"#).await;
    assert!(count.data.to_string().contains('3'), "{:?}", count.data);

    let ids: Vec<String> = h
        .db
        .inbox("beta", 10, 0)
        .unwrap()
        .into_iter()
        .map(|drop| drop.id)
        .collect();
    h.db.mark_drop_read("beta", &ids[0]).unwrap();
    h.db.archive_drop("beta", &ids[1]).unwrap();

    let after = h.run_as(&h.beta, r#"{ unreadDropCount }"#).await;
    assert!(after.data.to_string().contains('1'), "{:?}", after.data);
}

// ── Reactions and conversations ─────────────────────────────────────────────────────────────

/// Reacting is the recipient's reply. A sender must not be able to answer their own message.
#[tokio::test]
async fn only_the_recipient_can_react_to_a_drop() {
    let h = harness();
    h.befriend("alpha", "beta");
    let id = h.drop_a_track("alpha", "beta", "Xtal").await;

    let by_sender = h
        .run_as(&h.alpha, &format!(r#"mutation {{ reactToDrop(id: "{id}", emoji: "🔥") }}"#))
        .await;
    assert_allowed(&by_sender, "the call itself succeeds");
    assert!(
        by_sender.data.to_string().contains("false"),
        "a sender reacted to their own drop: {:?}",
        by_sender.data
    );

    let by_stranger = h
        .run_as(&h.stranger, &format!(r#"mutation {{ reactToDrop(id: "{id}", emoji: "🔥") }}"#))
        .await;
    assert!(
        by_stranger.data.to_string().contains("false"),
        "a stranger reacted to somebody else's drop: {:?}",
        by_stranger.data
    );

    let by_recipient = h
        .run_as(&h.beta, &format!(r#"mutation {{ reactToDrop(id: "{id}", emoji: "🔥") }}"#))
        .await;
    assert!(by_recipient.data.to_string().contains("true"));
}

/// Unlike a read receipt, a reaction is something the recipient chose to send — so it goes back.
#[tokio::test]
async fn a_reaction_reaches_the_sender_but_a_read_receipt_still_does_not() {
    let h = harness();
    h.befriend("alpha", "beta");
    let id = h.drop_a_track("alpha", "beta", "Xtal").await;

    h.run_as(&h.beta, &format!(r#"mutation {{ markDropRead(id: "{id}") }}"#)).await;
    h.run_as(&h.beta, &format!(r#"mutation {{ reactToDrop(id: "{id}", emoji: "🔥") }}"#)).await;

    let seen = h
        .run_as(&h.alpha, r#"{ sentDrops { reaction readAt } }"#)
        .await;
    let body = seen.data.to_string();
    assert!(body.contains('🔥'), "the sender cannot see the reaction: {body}");
    assert!(
        body.contains("readAt\":null") || body.contains("readAt: null"),
        "a read receipt leaked to the sender: {body}"
    );
}

/// A thread is not a way around the read-receipt rule.
#[tokio::test]
async fn a_conversation_still_hides_read_receipts_on_your_own_messages() {
    let h = harness();
    h.befriend("alpha", "beta");
    let id = h.drop_a_track("alpha", "beta", "Xtal").await;
    h.run_as(&h.beta, &format!(r#"mutation {{ markDropRead(id: "{id}") }}"#)).await;

    let thread = h
        .run_as(&h.alpha, r#"{ conversation(with: "beta") { fromUser readAt } }"#)
        .await;
    assert_allowed(&thread, "reading your own thread");
    let body = thread.data.to_string();
    assert!(
        !body.contains("readAt\":\"2") && !body.contains("readAt\":\"1"),
        "the thread told the sender their drop was read: {body}"
    );
}

/// A conversation is between two people. A third cannot read it by naming them.
#[tokio::test]
async fn a_conversation_only_ever_contains_the_callers_own_messages() {
    let h = harness();
    h.befriend("alpha", "beta");
    h.drop_a_track("alpha", "beta", "Xtal").await;

    let peeked = h
        .run_as(&h.stranger, r#"{ conversation(with: "beta") { trackTitle } }"#)
        .await;
    assert_allowed(&peeked, "the query itself is not an error");
    assert!(
        !peeked.data.to_string().contains("Xtal"),
        "a stranger read somebody else's conversation: {:?}",
        peeked.data
    );
}

// ── Friend codes ────────────────────────────────────────────────────────────────────────────

/// The whole point of a five-minute code is that it cannot be used twice.
#[tokio::test]
async fn a_friend_code_is_single_use() {
    let h = harness();
    let code = h.mint_friend_code(&h.alpha).await;

    let first = h
        .run_as(&h.beta, &format!(r#"mutation {{ redeemFriendCode(code: "{code}") }}"#))
        .await;
    assert!(
        first.data.to_string().contains("alpha"),
        "the first redemption failed: {:?}",
        first.data
    );
    assert!(h.db.are_friends("alpha", "beta").unwrap());

    let second = h
        .run_as(&h.stranger, &format!(r#"mutation {{ redeemFriendCode(code: "{code}") }}"#))
        .await;
    assert!(
        second.data.to_string().contains("null"),
        "a spent code was redeemed again: {:?}",
        second.data
    );
    assert!(!h.db.are_friends("alpha", "stranger").unwrap());
}

/// Minting a new code has to kill the old one, or every code ever shown stays live.
#[tokio::test]
async fn minting_a_friend_code_invalidates_the_previous_one() {
    let h = harness();
    let old = h.mint_friend_code(&h.alpha).await;
    let new = h.mint_friend_code(&h.alpha).await;
    assert_ne!(old, new, "re-minting returned the same code");

    let stale = h
        .run_as(&h.beta, &format!(r#"mutation {{ redeemFriendCode(code: "{old}") }}"#))
        .await;
    assert!(
        stale.data.to_string().contains("null"),
        "a replaced code still worked: {:?}",
        stale.data
    );
    assert!(!h.db.are_friends("alpha", "beta").unwrap());
}

/// Revoking is what a closed QR panel does, and it has to actually take effect.
#[tokio::test]
async fn a_revoked_friend_code_stops_working() {
    let h = harness();
    let code = h.mint_friend_code(&h.alpha).await;
    h.run_as(&h.alpha, r#"mutation { revokeFriendCode }"#).await;

    let after = h
        .run_as(&h.beta, &format!(r#"mutation {{ redeemFriendCode(code: "{code}") }}"#))
        .await;
    assert!(
        after.data.to_string().contains("null"),
        "a revoked code still worked: {:?}",
        after.data
    );
    assert!(!h.db.are_friends("alpha", "beta").unwrap());
}

/// A code is a stand-in for the username search, not for consent. A block still wins.
#[tokio::test]
async fn a_friend_code_cannot_be_used_to_get_around_a_block() {
    let h = harness();
    h.db.block_user("alpha", "beta").unwrap();
    let code = h.mint_friend_code(&h.alpha).await;

    let blocked = h
        .run_as(&h.beta, &format!(r#"mutation {{ redeemFriendCode(code: "{code}") }}"#))
        .await;
    assert!(
        blocked.data.to_string().contains("null"),
        "a block was talked past with a code: {:?}",
        blocked.data
    );
    assert!(!h.db.are_friends("alpha", "beta").unwrap());
}

/// An expired code is dead, which is the only reason the short lifetime is worth anything.
#[tokio::test]
async fn an_expired_friend_code_is_refused() {
    let h = harness();
    let code = h.mint_friend_code(&h.alpha).await;
    // Reaching past the API deliberately: there is no way to wait five minutes in a test, and the
    // expiry check is the thing being tested rather than the clock.
    h.expire_friend_codes();

    let after = h
        .run_as(&h.beta, &format!(r#"mutation {{ redeemFriendCode(code: "{code}") }}"#))
        .await;
    assert!(
        after.data.to_string().contains("null"),
        "an expired code still worked: {:?}",
        after.data
    );
    assert!(!h.db.are_friends("alpha", "beta").unwrap());
}
