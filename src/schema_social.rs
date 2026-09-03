//! Profiles, friends, presence and listen-along.
//!
//! Everything else in this server answers one question — "is the caller the account they named?" —
//! and `authorize` answers it by refusing whenever the answer is no. This module is the first
//! deliberate exception, and it is written to keep the exception as small as it can be.
//!
//! The rule is [`require_visible`]: a caller may read another account's data only when **both** of
//! two independent things hold. They are accepted friends, *and* the subject has opened the
//! specific surface being asked for. Friendship alone reveals nothing; the flags default closed and
//! are per-surface, so agreeing to be someone's friend is not agreeing to be watched.

use async_graphql::{Context, Object, SimpleObject};

use crate::db::Db;
use crate::db_social::{FriendState, Profile, LEGACY_DEVICE_ID};
use crate::schema::{bounded, caller, forbidden, normalise_username, require_admin, StatEntry};

/// Which of the subject's flags a read is gated on.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Surface {
    NowPlaying,
    Stats,
    /// The listening *history* — the activity feed and the circle recap. Distinct from `Stats`
    /// because an aggregate and a timeline are different disclosures: "40 hours of Aphex Twin" is
    /// a summary, "played this at 3am on Tuesday" is a record of a person's evenings.
    Activity,
}

/// The single gate for reading another account's data.
///
/// Reading your own data is always allowed and short-circuits: a private profile is private from
/// other people, not from its owner.
///
/// Every refusal is the same refusal. "Not your friend", "they have that switched off" and "no such
/// account" must be indistinguishable, or the error message becomes the very directory that
/// `discoverable` exists to let people stay out of.
pub(crate) fn require_visible(ctx: &Context<'_>, subject: &str, surface: Surface) -> async_graphql::Result<Profile> {
    let authed = caller(ctx)?;
    let db = ctx.data::<Db>()?;
    let subject = normalise_username(subject)?;

    let profile = db
        .profile(&subject)?
        .ok_or_else(|| forbidden("no such account, or it is not visible to you"))?;

    if authed.username().eq_ignore_ascii_case(&subject) {
        return Ok(profile);
    }
    if !db.are_friends(authed.username(), &subject)? {
        return Err(forbidden("no such account, or it is not visible to you"));
    }
    // Incognito is folded into each `shows_*` below, but refused up front as well: it makes the
    // rule visible at the gate, and it means a surface added here without a `shows_*` helper is
    // still closed rather than quietly open. The refusal is the one a stranger gets, so that
    // being quiet is itself undisclosed.
    if profile.incognito {
        return Err(forbidden("no such account, or it is not visible to you"));
    }
    let open = match surface {
        Surface::NowPlaying => profile.shows_now_playing(),
        Surface::Stats => profile.shows_stats(),
        Surface::Activity => profile.shows_activity(),
    };
    if open {
        Ok(profile)
    } else {
        Err(forbidden("no such account, or it is not visible to you"))
    }
}

#[derive(SimpleObject, Clone)]
pub struct ProfilePayload {
    pub username: String,
    pub display_name: Option<String>,
    pub bio: Option<String>,
    pub avatar_url: Option<String>,
    pub created_at: String,
    /// `none`, `pending`, `accepted` or `blocked`, from the viewer's side.
    pub friend_state: String,
    /// True when it was the viewer who sent an unanswered request. Meaningless unless
    /// `friend_state` is `pending`.
    pub outgoing: bool,
    pub show_now_playing: bool,
    pub show_stats: bool,
    pub discoverable: bool,
    pub share_library: bool,
    pub show_activity: bool,
    pub incognito: bool,
    /// Public identity key for E2EE track drops and communications.
    pub public_key: Option<String>,
}

/// What a client needs to finish enrolling a second factor. Shown once and never again.
#[derive(SimpleObject, Clone)]
pub struct TotpEnrolmentPayload {
    /// For the QR code.
    pub otpauth_uri: String,
    /// For typing in by hand when the camera will not cooperate.
    pub secret_base32: String,
}

/// The recovery codes, returned once when enrolment is confirmed.
#[derive(SimpleObject, Clone)]
pub struct TotpConfirmedPayload {
    /// Ten single-use codes. The server keeps only their digests and cannot show these again.
    pub recovery_codes: Vec<String>,
    /// How many device tokens were signed out by enrolling. See `confirmTotp`.
    pub devices_signed_out: i64,
}

/// The state of an account's second factor, for the settings screen.
#[derive(SimpleObject, Clone)]
pub struct TotpStatus {
    pub is_enabled: bool,
    /// Unused recovery codes left. Zero means the next lost phone is a lockout.
    pub recovery_codes_remaining: i64,
    /// False when the server has no `AGRO_SECRET_KEY`, so enrolment cannot be offered at all.
    pub is_available: bool,
}

/// What a friend is playing, projected from the one handoff row their account already keeps.
///
/// Presence needs no storage of its own: the handoff a device publishes so its *owner* can pick a
/// session up elsewhere is the same fact a friend is being shown.
#[derive(SimpleObject, Clone)]
pub struct FriendNowPlaying {
    pub username: String,
    pub track_uri: String,
    pub track_title: String,
    pub artist_name: String,
    pub album_name: Option<String>,
    pub artwork_url: Option<String>,
    pub position_ms: i64,
    pub is_playing: bool,
    pub updated_at: String,
    /// Which of the host's devices is playing it.
    ///
    /// A listener needs this to ask for the audio at all: both the direct transfer and the relay
    /// address one device, not an account.
    pub device_id: String,
    /// SHA-256 of the file the host is playing, when they have one. `None` for anything streamed,
    /// which is what sends a listener back to matching by name.
    pub content_hash: Option<String>,
    /// Where to reach the host's device directly, and the token to present when doing so.
    ///
    /// Both are `Some` only when the two devices look like they share a local network *and* the
    /// viewer is allowed to see this account at all. Neither is the authorisation on its own — see
    /// [`crate::ws::WsHub::same_network`] for why the address is a hint and the token is the gate.
    pub peer_lan_address: Option<String>,
    pub peer_lan_token: Option<String>,
}

#[derive(SimpleObject, Clone)]
pub struct FriendEdgePayload {
    pub profile: ProfilePayload,
    pub now_playing: Option<FriendNowPlaying>,
}

/// How much two accounts' listening overlaps.
#[derive(SimpleObject, Clone)]
pub struct TasteMatch {
    /// 0–100. See `taste_score` for what it actually measures.
    pub score: i64,
    pub shared_artists: Vec<StatEntry>,
    pub shared_tracks: Vec<StatEntry>,
}

#[derive(SimpleObject, Clone)]
pub struct ListenAlongPayload {
    pub host: String,
    pub listeners: Vec<String>,
    pub now_playing: Option<FriendNowPlaying>,
}

/// One device's published identity key.
///
/// The device id is opaque to the server and meaningful only within the account — it exists so a
/// sender can seal one copy per device and a device can recognise its own copy on the way back.
#[derive(SimpleObject, Clone, Debug)]
pub struct DeviceKeyPayload {
    pub device_id: String,
    pub public_key: String,
}

/// A friend code as a client renders it.
///
/// [`ttl_seconds`] is sent rather than left for the client to derive from [`expires_at`], because
/// the client's clock is not the server's and a QR panel that re-mints on a drifting timer would
/// either show a dead code or thrash. It counts down from *now* on the device that asked.
#[derive(SimpleObject, Clone, Debug)]
pub struct FriendCodePayload {
    pub code: String,
    pub expires_at: String,
    pub ttl_seconds: i64,
}

/// Short enough that a code photographed off a screen is dead before the photographer gets home,
/// long enough to find the camera and scan it.
const FRIEND_CODE_TTL_MINUTES: i64 = 5;

#[derive(SimpleObject, Clone)]
pub struct InvitePayload {
    pub code: String,
    pub created_by: String,
    pub created_at: String,
    pub expires_at: Option<String>,
    pub max_uses: i64,
    pub used_count: i64,
    pub revoked: bool,
}

pub fn profile_payload(profile: &Profile, state: Option<FriendState>, outgoing: bool) -> ProfilePayload {
    ProfilePayload {
        username: profile.username.clone(),
        display_name: profile.display_name.clone(),
        bio: profile.bio.clone(),
        avatar_url: profile.avatar_url.clone(),
        created_at: profile.created_at.clone(),
        friend_state: match state {
            Some(FriendState::Accepted) => "accepted",
            Some(FriendState::Pending) => "pending",
            // A block is never disclosed to the account it was applied to. From their side this is
            // simply somebody they are not friends with.
            Some(FriendState::Blocked) | None => "none",
        }
        .to_string(),
        outgoing,
        show_now_playing: profile.show_now_playing,
        show_stats: profile.show_stats,
        discoverable: profile.discoverable,
        share_library: profile.share_library,
        show_activity: profile.show_activity,
        incognito: profile.incognito,
        public_key: profile.public_key.clone(),
    }
}

/// How long after its last update a handoff still counts as "now".
///
/// Clients republish every ten seconds while playing, so anything older than this is a session
/// that ended without saying so — the app was killed, the phone lost signal, the battery went.
const NOW_PLAYING_STALE_AFTER_SECS: i64 = 300;

/// What a friend is playing *at the moment*, or `None`.
///
/// The handoff row is durable — it is what lets you resume on another device hours later — so it
/// describes the last thing played, not the thing being played. Reading it directly meant a friend
/// who listened days ago sat permanently in "Listening now" with a track long finished.
///
/// Two conditions, both necessary: they were playing rather than paused when they last reported,
/// and they reported recently enough for that to still be true.
fn live_now_playing(db: &Db, username: &str) -> async_graphql::Result<Option<FriendNowPlaying>> {
    let Some(now) = now_playing_of(db, username)? else {
        return Ok(None);
    };
    if !now.is_playing {
        return Ok(None);
    }
    let fresh = chrono::DateTime::parse_from_rfc3339(&now.updated_at)
        .map(|seen| {
            (chrono::Utc::now() - seen.with_timezone(&chrono::Utc)).num_seconds()
                < NOW_PLAYING_STALE_AFTER_SECS
        })
        // An unparseable timestamp is treated as stale: showing something that may be days old is
        // the failure this exists to prevent.
        .unwrap_or(false);
    Ok(if fresh { Some(now) } else { None })
}

/// The handoff row as a friend may see it, or `None` when there is nothing to show.
fn now_playing_of(db: &Db, username: &str) -> async_graphql::Result<Option<FriendNowPlaying>> {
    let Some(handoff) = db.get_handoff(username)? else {
        return Ok(None);
    };
    Ok(Some(FriendNowPlaying {
        username: username.to_string(),
        track_uri: handoff.track_uri,
        track_title: handoff.track_title,
        artist_name: handoff.artist_name,
        album_name: handoff.album_name,
        artwork_url: handoff.artwork_url,
        position_ms: handoff.position_ms,
        is_playing: handoff.is_playing,
        updated_at: handoff.updated_at,
        device_id: handoff.device_id,
        content_hash: handoff.content_hash,
        // No viewer, no pairwise answer. Whether a direct transfer is possible is a fact about two
        // devices, so a projection that does not know who is asking cannot fill these in.
        peer_lan_address: None,
        peer_lan_token: None,
    }))
}

/// The same, answered for a specific viewer on a specific device.
///
/// The direct-transfer fields are pairwise — they depend on where *both* devices are — so they can
/// only be filled in once the asker is known. Everything else is identical, and a viewer who
/// cannot be told about this host never reaches here: the caller has already applied the
/// friendship and visibility gates.
/// The identity keys an account has published, for binding into a P2P grant.
///
/// Answers an empty list when the lookup fails rather than propagating. A grant that cannot name
/// the listener's keys is the pre-registry behaviour — the host falls back to the request header —
/// and losing the ability to listen along because a key lookup errored would be the worse failure.
pub(crate) fn published_keys(db: &Db, username: &str) -> Vec<String> {
    db.device_keys_for(username)
        .map(|keys| keys.into_iter().map(|k| k.public_key).collect())
        .unwrap_or_default()
}

fn now_playing_for_viewer(
    db: &Db,
    ws_hub: &crate::ws::WsHub,
    host: &str,
    viewer: &str,
) -> async_graphql::Result<Option<FriendNowPlaying>> {
    let Some(mut now) = now_playing_of(db, host)? else {
        return Ok(None);
    };
    if ws_hub.shares_network_with_user(host, &now.device_id, viewer) {
        if let Some(lan) = ws_hub.get_lan_address(host, &now.device_id) {
            let viewer_keys = published_keys(db, viewer);
            now.peer_lan_token =
                ws_hub.grant_p2p_token(host, &now.device_id, viewer, &viewer_keys);
            // The address is only worth handing over alongside a token to use it with.
            if now.peer_lan_token.is_some() {
                now.peer_lan_address = Some(lan);
            }
        }
    }
    Ok(Some(now))
}

#[derive(Default)]
pub struct SocialQuery;

#[Object]
impl SocialQuery {
    /// Whether the signed-in account is currently quiet.
    ///
    /// Only ever answered for the caller. There is deliberately no version of this question about
    /// somebody else: an incognito account is not returned to anyone at all, and saying "they have
    /// gone quiet" would be exactly the disclosure incognito exists to prevent. A friend meets the
    /// same refusal a stranger does.
    ///
    /// Needed because [`Mutation::set_incognito`] returns the new value but nothing could *read*
    /// it — a client that has just launched has no way to learn the account is already quiet, and
    /// would carry on recording until someone touched the switch.
    async fn incognito(&self, ctx: &Context<'_>) -> async_graphql::Result<bool> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        Ok(db
            .profile(authed.username())?
            .ok_or_else(|| forbidden("this account no longer exists"))?
            .incognito)
    }

    /// The state of the caller's second factor.
    async fn totp_status(&self, ctx: &Context<'_>) -> async_graphql::Result<TotpStatus> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        Ok(TotpStatus {
            is_enabled: db.totp_is_confirmed(authed.username())?,
            recovery_codes_remaining: db.recovery_codes_remaining(authed.username())?,
            is_available: crate::totp::is_configured(),
        })
    }

    /// Whether the signed-in account has active 2FA / TOTP configured.
    async fn has_totp(&self, ctx: &Context<'_>) -> async_graphql::Result<bool> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        let has = db.totp_is_confirmed(authed.username())
            .map_err(|e| async_graphql::Error::new(format!("Failed to check TOTP: {e}")))?;
        Ok(has)
    }

    /// One account, as the caller is allowed to see it.
    ///
    /// Not gated by [`require_visible`]: the card itself — name, bio, avatar — is what someone
    /// needs in order to decide whether to send a request, so it is readable by any signed-in
    /// account for any discoverable or already-connected profile. What the flags gate is the
    /// *contents*: now playing and stats have their own resolvers and their own checks.
    async fn profile(
        &self,
        ctx: &Context<'_>,
        username: String,
    ) -> async_graphql::Result<Option<ProfilePayload>> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        let subject = normalise_username(&username)?;

        let Some(profile) = db.profile(&subject)? else {
            return Ok(None);
        };
        let state = db.friend_state(authed.username(), &subject)?;

        // Someone who is neither discoverable nor connected to the caller is not theirs to look up.
        let reachable = authed.username().eq_ignore_ascii_case(&subject)
            || profile.discoverable
            || matches!(state, Some(FriendState::Accepted) | Some(FriendState::Pending));
        if !reachable || state == Some(FriendState::Blocked) {
            return Ok(None);
        }

        let outgoing = db
            .outgoing_requests(authed.username())?
            .iter()
            .any(|e| e.profile.username.eq_ignore_ascii_case(&subject));
        Ok(Some(profile_payload(&profile, state, outgoing)))
    }

    /// Every identity key `username` has published, one per device.
    ///
    /// Gated on friendship with the same refusal `dropTrack` uses, and for the same reason: a key
    /// list that answered differently for a stranger and for an account that does not exist would
    /// be a way to probe for accounts. Reading your own list is always allowed — a client needs it
    /// to seal a copy to its own other devices.
    ///
    /// A sender seals one copy per entry here. An account with no entries is one whose devices
    /// have never published a key, and the caller should fall back to sending the note in clear or
    /// not at all — never to sealing it to nobody.
    async fn device_keys(
        &self,
        ctx: &Context<'_>,
        username: String,
    ) -> async_graphql::Result<Vec<DeviceKeyPayload>> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        let subject = normalise_username(&username)?;

        if !authed.username().eq_ignore_ascii_case(&subject)
            && !db.are_friends(authed.username(), &subject)?
        {
            return Err(forbidden("no such account, or it is not visible to you"));
        }

        Ok(db
            .device_keys_for(&subject)?
            .into_iter()
            .map(|key| DeviceKeyPayload {
                device_id: key.device_id,
                public_key: key.public_key,
            })
            .collect())
    }

    /// The public directory. Only accounts that asked to be listed appear in it.
    async fn search_users(
        &self,
        ctx: &Context<'_>,
        query: String,
        limit: Option<i64>,
    ) -> async_graphql::Result<Vec<ProfilePayload>> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;

        let found = db.search_users(authed.username(), &query, limit.unwrap_or(20))?;
        let mut payloads = Vec::new();
        for profile in found {
            let state = db.friend_state(authed.username(), &profile.username)?;
            // A block removes the account from results in both directions. Someone the caller
            // blocked should not resurface, and someone who blocked the caller should not be
            // findable by them.
            if state == Some(FriendState::Blocked) {
                continue;
            }
            payloads.push(profile_payload(&profile, state, false));
        }
        Ok(payloads)
    }

    /// Accepted friends, each with what they are playing when they allow that to be seen.
    async fn friends(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<FriendEdgePayload>> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;

        let mut edges = Vec::new();
        for profile in db.friends(authed.username())? {
            // Not an error when it is closed — a friend who has not opted in is simply a friend
            // with nothing showing, which is different from a friend who is offline.
            let now_playing = if profile.shows_now_playing() {
                live_now_playing(db, &profile.username)?
            } else {
                None
            };
            edges.push(FriendEdgePayload {
                profile: profile_payload(&profile, Some(FriendState::Accepted), false),
                now_playing,
            });
        }
        Ok(edges)
    }

    /// Requests in both directions: ones to answer, and ones already sent.
    async fn friend_requests(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<ProfilePayload>> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;

        let mut payloads: Vec<ProfilePayload> = db
            .incoming_requests(authed.username())?
            .iter()
            .map(|e| profile_payload(&e.profile, Some(FriendState::Pending), false))
            .collect();
        payloads.extend(
            db.outgoing_requests(authed.username())?
                .iter()
                .map(|e| profile_payload(&e.profile, Some(FriendState::Pending), true)),
        );
        Ok(payloads)
    }

    /// Just the presence feed: friends who are playing something and allow it to be seen.
    async fn friends_now_playing(
        &self,
        ctx: &Context<'_>,
    ) -> async_graphql::Result<Vec<FriendNowPlaying>> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;

        let mut playing = Vec::new();
        for profile in db.friends(authed.username())? {
            if !profile.shows_now_playing() {
                continue;
            }
            if let Some(now) = live_now_playing(db, &profile.username)? {
                playing.push(now);
            }
        }
        Ok(playing)
    }

    /// How much the caller's listening overlaps a friend's. Gated on the friend's stats flag.
    async fn taste_match(
        &self,
        ctx: &Context<'_>,
        username: String,
    ) -> async_graphql::Result<TasteMatch> {
        let authed = caller(ctx)?;
        let subject = require_visible(ctx, &username, Surface::Stats)?;
        let db = ctx.data::<Db>()?;
        let now = chrono::Utc::now().timestamp();

        let mine = crate::stats::compute(&db.scrobble_rows(authed.username(), None, None)?, 50, now);
        let theirs =
            crate::stats::compute(&db.scrobble_rows(&subject.username, None, None)?, 50, now);

        Ok(taste_match(&mine, &theirs))
    }

    /// Who the caller is currently tuned in to, if anyone.
    async fn listen_along(
        &self,
        ctx: &Context<'_>,
    ) -> async_graphql::Result<Option<ListenAlongPayload>> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;

        let Some(session) = db.listen_along_of(authed.username())? else {
            return Ok(None);
        };
        // The friendship may have been withdrawn since the session started, in which case the row
        // is stale and the session is over.
        if require_visible(ctx, &session.host, Surface::NowPlaying).is_err() {
            db.clear_listen_along(authed.username())?;
            return Ok(None);
        }
        let ws_hub = ctx.data::<std::sync::Arc<crate::ws::WsHub>>()?;
        Ok(Some(ListenAlongPayload {
            listeners: db.listeners_of(&session.host)?,
            now_playing: now_playing_for_viewer(db, ws_hub, &session.host, authed.username())?,
            host: session.host,
        }))
    }

    /// Accounts waiting to be let in. The approval queue, for the dashboard.
    async fn pending_accounts(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<ProfilePayload>> {
        require_admin(ctx)?;
        let db = ctx.data::<Db>()?;
        Ok(db
            .pending_accounts()?
            .iter()
            .map(|p| profile_payload(p, None, false))
            .collect())
    }

    async fn invites(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<InvitePayload>> {
        require_admin(ctx)?;
        let db = ctx.data::<Db>()?;
        Ok(db
            .list_invites()?
            .into_iter()
            .map(|i| InvitePayload {
                code: i.code,
                created_by: i.created_by,
                created_at: i.created_at,
                expires_at: i.expires_at,
                max_uses: i.max_uses,
                used_count: i.used_count,
                revoked: i.revoked,
            })
            .collect())
    }
}

/// Scores the overlap between two listeners.
///
/// The score is the share of the *smaller* history that both accounts have in common, by artist —
/// a Szymkiewicz–Simpson overlap rather than a Jaccard one. With Jaccard, someone with 40 artists
/// and someone with 4000 could never score highly no matter how completely the first was contained
/// in the second, which reads as "you have nothing in common" when the truth is the opposite.
///
/// Play counts are deliberately ignored. Whether two people share a taste is a question about
/// which artists they both listen to, not about who listens more.
pub(crate) fn taste_match(mine: &crate::stats::Stats, theirs: &crate::stats::Stats) -> TasteMatch {
    let shared_artists = shared(&mine.top_artists, &theirs.top_artists);
    let shared_tracks = shared(&mine.top_tracks, &theirs.top_tracks);

    let smaller = mine.top_artists.len().min(theirs.top_artists.len());
    let score = if smaller == 0 {
        0
    } else {
        ((shared_artists.len() as f64 / smaller as f64) * 100.0).round() as i64
    };

    TasteMatch {
        score,
        shared_artists,
        shared_tracks,
    }
}

/// The names present in both lists, carrying the *caller's* play count for each.
fn shared(mine: &[(String, i64)], theirs: &[(String, i64)]) -> Vec<StatEntry> {
    mine.iter()
        .filter(|(name, _)| {
            theirs
                .iter()
                .any(|(other, _)| other.eq_ignore_ascii_case(name))
        })
        .map(|(name, value)| StatEntry {
            name: name.clone(),
            value: *value,
        })
        .collect()
}

#[derive(Default)]
pub struct SocialMutation;

#[Object]
impl SocialMutation {
    /// Edits the caller's own profile. Fields left out are left alone rather than blanked.
    async fn update_profile(
        &self,
        ctx: &Context<'_>,
        display_name: Option<String>,
        bio: Option<String>,
        avatar_url: Option<String>,
    ) -> async_graphql::Result<ProfilePayload> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;

        // Every one of these is stored and rendered on someone else's screen.
        let display_name = display_name.map(|v| bounded(&v, 64, "Display name")).transpose()?;
        let bio = bio.map(|v| bounded(&v, 280, "Bio")).transpose()?;
        let avatar_url = avatar_url.map(|v| validated_avatar(&v)).transpose()?;

        db.update_profile(
            authed.username(),
            display_name.as_deref(),
            bio.as_deref(),
            avatar_url.as_deref(),
        )?;
        let profile = db
            .profile(authed.username())?
            .ok_or_else(|| forbidden("this account no longer exists"))?;
        Ok(profile_payload(&profile, None, false))
    }

    /// Publishes or withdraws one device's identity key.
    ///
    /// `device_id` names which of the caller's devices this key belongs to, and only that entry is
    /// touched. Omitting it writes the `legacy` entry — which is what a client that predates the
    /// registry is doing whether it knows it or not, and it is the behaviour those clients already
    /// had: one key for the account.
    ///
    /// `users.public_key` is still written, because older clients read it and nothing else. It is
    /// last-writer-wins, so it will hold whichever device published most recently; that is
    /// tolerable now only because nothing that seals a note reads it any more.
    async fn set_public_key(
        &self,
        ctx: &Context<'_>,
        public_key: Option<String>,
        device_id: Option<String>,
    ) -> async_graphql::Result<ProfilePayload> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        let key = public_key.as_deref().map(str::trim).filter(|s| !s.is_empty());
        if let Some(k) = key {
            if k.len() > 512 {
                return Err("Public key is too long (max 512 bytes)".into());
            }
        }

        let device = match device_id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(id) => bounded(id, 128, "deviceId")?,
            None => LEGACY_DEVICE_ID.to_string(),
        };
        match key {
            Some(k) => db.register_device_key(authed.username(), &device, k)?,
            // Withdrawing removes this device's entry only. The other devices on the account keep
            // receiving sealed messages, which is the entire point of the registry.
            None => {
                db.forget_device_key(authed.username(), &device)?;
            }
        }

        db.set_public_key(authed.username(), key)?;
        let profile = db
            .profile(authed.username())?
            .ok_or_else(|| forbidden("this account no longer exists"))?;
        Ok(profile_payload(&profile, None, false))
    }

    /// Begins enrolling a second factor for the caller.
    ///
    /// Writes a secret that is not yet binding: nothing enforces it until `confirmTotp` has seen a
    /// code computed from it. That ordering is deliberate — the failure mode of a second factor is
    /// always lockout, so the recoverable sequence is the only one on offer.
    async fn begin_totp(&self, ctx: &Context<'_>) -> async_graphql::Result<TotpEnrolmentPayload> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        let enrolment = db
            .begin_totp_enrolment(authed.username())
            .map_err(async_graphql::Error::new)?;
        Ok(TotpEnrolmentPayload {
            otpauth_uri: enrolment.otpauth_uri,
            secret_base32: enrolment.secret_base32,
        })
    }

    /// Confirms the pending enrolment with a code from the authenticator.
    ///
    /// **This signs out every other device**, and returns how many. Whoever held the passphrase
    /// before this moment has already traded it for tokens, and those tokens do not carry a second
    /// factor — leaving them alive would make enrolling change nothing for the attacker it is meant
    /// to shut out. The device doing the enrolling is spared, so the user is not signed out of the
    /// screen they are standing on.
    async fn confirm_totp(
        &self,
        ctx: &Context<'_>,
        code: String,
    ) -> async_graphql::Result<TotpConfirmedPayload> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        let before = db.list_app_passwords(authed.username())?.len() as i64;
        let spare = Some(authed.token_hash.as_str()).filter(|h| !h.is_empty());
        let recovery_codes = db
            .confirm_totp_enrolment(authed.username(), &code, spare)
            .map_err(async_graphql::Error::new)?;
        let after = db.list_app_passwords(authed.username())?.len() as i64;

        db.record_event(
            crate::audit::Event::TotpEnrolled,
            crate::audit::Record::new()
                .user(authed.username())
                .device(authed.device_label.clone()),
        );
        Ok(TotpConfirmedPayload {
            recovery_codes,
            devices_signed_out: (before - after).max(0),
        })
    }

    /// Turns the second factor off. Requires a current code, or a recovery code.
    ///
    /// The proof is the entire point. Without it, a stolen device token — which by construction did
    /// not pass a second factor — could remove the second factor, and the feature would protect
    /// nothing but itself. An administrator on a server that requires 2FA cannot disable it at all.
    async fn disable_totp(&self, ctx: &Context<'_>, code: String) -> async_graphql::Result<bool> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;

        if authed.is_admin() && crate::auth::admin_totp_required() {
            return Err(forbidden(
                "this server requires administrators to keep two-factor authentication enabled",
            ));
        }

        let outcome = db.verify_totp(authed.username(), &code)?;
        if !outcome.is_satisfied() {
            db.record_event(
                crate::audit::Event::TotpFailed,
                crate::audit::Record::new()
                    .user(authed.username())
                    .detail("while trying to disable the second factor"),
            );
            return Err("That code is not right.".into());
        }

        db.disable_totp(authed.username())?;
        db.record_event(
            crate::audit::Event::TotpDisabled,
            crate::audit::Record::new()
                .user(authed.username())
                .device(authed.device_label.clone()),
        );
        Ok(true)
    }

    /// Issues a fresh set of recovery codes, invalidating the old ones. Requires a current code.
    async fn regenerate_recovery_codes(
        &self,
        ctx: &Context<'_>,
        code: String,
    ) -> async_graphql::Result<Vec<String>> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        let outcome = db.verify_totp(authed.username(), &code)?;
        if !outcome.is_satisfied() {
            return Err("That code is not right.".into());
        }
        let codes = db
            .regenerate_recovery_codes(authed.username())
            .map_err(async_graphql::Error::new)?;
        db.record_event(
            crate::audit::Event::TotpEnrolled,
            crate::audit::Record::new()
                .user(authed.username())
                .detail("recovery codes regenerated"),
        );
        Ok(codes)
    }

    /// The three switches that decide what a friend can see, and whether strangers can find you.
    ///
    /// One mutation rather than three, so the privacy screen writes what the user sees in a single
    /// round trip and cannot land half-applied.
    /// Each switch is optional and one left out is left alone.
    ///
    /// This matters more than it looks: the flags are three independent decisions, and requiring
    /// all three on every call means a client flipping one has to resend its idea of the other two.
    /// Two devices doing that concurrently silently undo each other — a switch turned on over here
    /// gets reverted by a stale copy sent from over there.
    async fn set_visibility(
        &self,
        ctx: &Context<'_>,
        show_now_playing: Option<bool>,
        show_stats: Option<bool>,
        discoverable: Option<bool>,
        share_library: Option<bool>,
        show_activity: Option<bool>,
    ) -> async_graphql::Result<ProfilePayload> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;

        let current = db
            .profile(authed.username())?
            .ok_or_else(|| forbidden("this account no longer exists"))?;
        let show_now_playing = show_now_playing.unwrap_or(current.show_now_playing);
        let show_stats = show_stats.unwrap_or(current.show_stats);
        let discoverable = discoverable.unwrap_or(current.discoverable);

        db.set_visibility(authed.username(), show_now_playing, show_stats)?;
        db.set_discoverable(authed.username(), discoverable)?;
        if let Some(share) = share_library {
            db.set_share_library(authed.username(), share)?;
        }
        if let Some(show) = show_activity {
            db.set_show_activity(authed.username(), show)?;
        }

        // Turning now-playing off ends every session following it. Leaving them attached would mean
        // a switch the user just set to "off" still feeding the thing it was meant to stop.
        if !show_now_playing {
            for listener in db.listeners_of(authed.username())? {
                db.clear_listen_along(&listener)?;
                ctx.data::<std::sync::Arc<crate::ws::WsHub>>()?.notify_user(
                    &listener,
                    "LISTEN_ALONG",
                    serde_json::json!({ "stopped": true, "host": authed.username() }),
                );
            }
        }

        let profile = db
            .profile(authed.username())?
            .ok_or_else(|| forbidden("this account no longer exists"))?;
        Ok(profile_payload(&profile, None, false))
    }

    /// Goes quiet, or stops being quiet.
    ///
    /// Its own mutation rather than a sixth flag on `set_visibility`, because it is a different
    /// kind of thing: the others are standing consents, this is a temporary override of all of
    /// them. Bundling it would mean leaving incognito had to restore the rest from a client's idea
    /// of what they were, and a stale copy would silently turn a privacy switch back on.
    ///
    /// The server is the authority. Every client reads this value rather than keeping its own, so
    /// one device going quiet takes the whole account quiet — a phone still announcing the account
    /// from a pocket is exactly the failure a device-local incognito has.
    async fn set_incognito(
        &self,
        ctx: &Context<'_>,
        incognito: bool,
    ) -> async_graphql::Result<bool> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;

        db.set_incognito(authed.username(), incognito)?;

        // Going quiet ends every session following this account, for the same reason turning
        // now-playing off does: a listener left attached keeps receiving the thing incognito was
        // turned on to stop.
        if incognito {
            for listener in db.listeners_of(authed.username())? {
                db.clear_listen_along(&listener)?;
                ctx.data::<std::sync::Arc<crate::ws::WsHub>>()?.notify_user(
                    &listener,
                    "LISTEN_ALONG",
                    serde_json::json!({ "stopped": true, "host": authed.username() }),
                );
            }
        }

        // Tells this account's *own* devices, so the switch appears flipped on all of them rather
        // than only on the one it was touched from. Friends are told nothing: that an account went
        // quiet is itself a disclosure, and the refusal they meet is the same one a stranger gets.
        ctx.data::<std::sync::Arc<crate::ws::WsHub>>()?.notify_user(
            authed.username(),
            "SETTINGS_SYNC",
            serde_json::json!({ "incognito": incognito }),
        );

        Ok(incognito)
    }

    /// Asks to be someone's friend.
    ///
    /// Answers `true` for a request that was recorded and `false` for one that was not, without
    /// saying which of the several reasons applied — an existing edge, a block, or no such account
    /// all look the same from here, and telling them apart would let this be used to probe for
    /// accounts that chose not to be listed.
    async fn send_friend_request(
        &self,
        ctx: &Context<'_>,
        username: String,
    ) -> async_graphql::Result<bool> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        let subject = normalise_username(&username)?;

        let Some(profile) = db.profile(&subject)? else {
            return Ok(false);
        };
        // Reachable by the same rule `profile` uses: listed, or already connected.
        let state = db.friend_state(authed.username(), &subject)?;
        if state.is_some() || !profile.discoverable {
            return Ok(false);
        }

        let sent = db.send_friend_request(authed.username(), &subject)?;
        if sent {
            ctx.data::<std::sync::Arc<crate::ws::WsHub>>()?.notify_user(
                &subject,
                "FRIEND_REQUEST",
                serde_json::json!({ "from": authed.username() }),
            );
        }
        Ok(sent)
    }

    async fn accept_friend_request(
        &self,
        ctx: &Context<'_>,
        username: String,
    ) -> async_graphql::Result<bool> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        let subject = normalise_username(&username)?;

        let accepted = db.accept_friend_request(authed.username(), &subject)?;
        if accepted {
            ctx.data::<std::sync::Arc<crate::ws::WsHub>>()?.notify_user(
                &subject,
                "FRIEND_REQUEST",
                serde_json::json!({ "accepted_by": authed.username() }),
            );
        }
        Ok(accepted)
    }

    /// Mints a short-lived code for adding the caller as a friend in person.
    ///
    /// For the case the username search cannot serve: two people in the same room, one of whom is
    /// not `discoverable` and should not have to become so just to be added once. The code stands
    /// in for the search, not for the consent — redeeming it still produces the same friend edge
    /// the ordinary flow does.
    ///
    /// Any previous code for this account is dropped when a new one is minted, so only the code
    /// currently on screen works. Clients are expected to re-mint every few minutes while the
    /// panel is open and to call `revokeFriendCode` when it closes.
    async fn create_friend_code(&self, ctx: &Context<'_>) -> async_graphql::Result<FriendCodePayload> {
        let authed = caller(ctx)?;
        let code = ctx
            .data::<Db>()?
            .create_friend_code(authed.username(), FRIEND_CODE_TTL_MINUTES)?;
        Ok(FriendCodePayload {
            code: code.code,
            expires_at: code.expires_at,
            ttl_seconds: FRIEND_CODE_TTL_MINUTES * 60,
        })
    }

    /// Drops the caller's outstanding code, for a panel being closed or the app going away.
    async fn revoke_friend_code(&self, ctx: &Context<'_>) -> async_graphql::Result<bool> {
        let authed = caller(ctx)?;
        ctx.data::<Db>()?.revoke_friend_codes(authed.username())?;
        Ok(true)
    }

    /// Spends a code and becomes friends with whoever minted it.
    ///
    /// Both directions at once, not a request to be accepted: the two people were standing
    /// together and one of them showed the other a screen, so the consent has already happened in
    /// a way a notification cannot improve on.
    ///
    /// Returns the username on success and null for every failure — unknown, expired, already
    /// spent, or the caller's own code. Distinguishing them would turn this into an oracle for
    /// which codes have existed.
    async fn redeem_friend_code(
        &self,
        ctx: &Context<'_>,
        code: String,
    ) -> async_graphql::Result<Option<String>> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;

        let Some(owner) = db.redeem_friend_code(&code)? else {
            return Ok(None);
        };
        // Redeeming your own code is a no-op rather than an error, and the code is spent either
        // way — a screen scanned by its own owner should not stay live for the next person.
        if owner.eq_ignore_ascii_case(authed.username()) {
            return Ok(None);
        }
        // A block from either side comes back as `Blocked` from `friend_state`, which is the one
        // state a code must not be able to talk its way past.
        if db.friend_state(authed.username(), &owner)? == Some(FriendState::Blocked) {
            return Ok(None);
        }

        // Two halves of one handshake. `send_friend_request` then `accept_friend_request` reuses
        // the paths that already know how to build the edge, rather than writing a third one that
        // has to agree with them.
        db.send_friend_request(&owner, authed.username())?;
        let accepted = db.accept_friend_request(authed.username(), &owner)?;
        if !accepted {
            return Ok(None);
        }

        ctx.data::<std::sync::Arc<crate::ws::WsHub>>()?.notify_user(
            &owner,
            "FRIEND_REQUEST",
            serde_json::json!({ "accepted_by": authed.username() }),
        );
        Ok(Some(owner))
    }

    /// Declines a request, or ends a friendship. The same operation on the data either way.
    async fn remove_friend(
        &self,
        ctx: &Context<'_>,
        username: String,
    ) -> async_graphql::Result<bool> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        let subject = normalise_username(&username)?;

        // Whoever is following whom, the session cannot outlive the friendship that permitted it.
        db.clear_listen_along(authed.username())?;
        db.clear_listen_along(&subject)?;
        let removed = db.remove_friend(authed.username(), &subject)?;
        if removed {
            // Tells the other side, so a request they sent stops sitting in their outgoing list
            // waiting for an answer that already happened.
            ctx.data::<std::sync::Arc<crate::ws::WsHub>>()?.notify_user(
                &subject,
                "FRIEND_REQUEST",
                serde_json::json!({ "declined_by": authed.username() }),
            );
        }
        Ok(removed)
    }

    async fn block_user(&self, ctx: &Context<'_>, username: String) -> async_graphql::Result<bool> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        let subject = normalise_username(&username)?;

        db.clear_listen_along(authed.username())?;
        db.clear_listen_along(&subject)?;
        db.block_user(authed.username(), &subject).map_err(Into::into)
    }

    async fn unblock_user(&self, ctx: &Context<'_>, username: String) -> async_graphql::Result<bool> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        let subject = normalise_username(&username)?;
        db.unblock_user(authed.username(), &subject).map_err(Into::into)
    }

    /// Tunes the caller in to a friend's playback.
    ///
    /// Gated on the host's now-playing flag, because following someone's playback is a strictly
    /// larger thing than seeing it, and there is no sensible reading in which the smaller one is
    /// closed and the larger one is open.
    async fn start_listen_along(
        &self,
        ctx: &Context<'_>,
        host: String,
    ) -> async_graphql::Result<ListenAlongPayload> {
        let authed = caller(ctx)?;
        let subject = require_visible(ctx, &host, Surface::NowPlaying)?;
        let db = ctx.data::<Db>()?;

        if subject.username.eq_ignore_ascii_case(authed.username()) {
            return Err("You are already listening to yourself".into());
        }

        db.set_listen_along(authed.username(), &subject.username)?;
        let ws_hub = ctx.data::<std::sync::Arc<crate::ws::WsHub>>()?;
        let payload = ListenAlongPayload {
            listeners: db.listeners_of(&subject.username)?,
            now_playing: now_playing_for_viewer(
                db,
                ws_hub,
                &subject.username,
                authed.username(),
            )?,
            host: subject.username.clone(),
        };

        // The host is told who joined, so their client can show the count without polling.
        ctx.data::<std::sync::Arc<crate::ws::WsHub>>()?.notify_user(
            &subject.username,
            "LISTEN_ALONG",
            serde_json::json!({
                "host": subject.username,
                "listeners": payload.listeners,
            }),
        );
        Ok(payload)
    }

    async fn stop_listen_along(&self, ctx: &Context<'_>) -> async_graphql::Result<bool> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;

        let host = db.listen_along_of(authed.username())?.map(|s| s.host);
        let stopped = db.clear_listen_along(authed.username())?;
        if let Some(host) = host {
            ctx.data::<std::sync::Arc<crate::ws::WsHub>>()?.notify_user(
                &host,
                "LISTEN_ALONG",
                serde_json::json!({ "host": host, "listeners": db.listeners_of(&host)? }),
            );
        }
        Ok(stopped)
    }

    async fn create_invite(
        &self,
        ctx: &Context<'_>,
        max_uses: Option<i64>,
        ttl_hours: Option<i64>,
    ) -> async_graphql::Result<InvitePayload> {
        let authed = require_admin(ctx)?;
        let db = ctx.data::<Db>()?;
        let invite = db.create_invite(authed.username(), max_uses.unwrap_or(1), ttl_hours)?;
        Ok(InvitePayload {
            code: invite.code,
            created_by: invite.created_by,
            created_at: invite.created_at,
            expires_at: invite.expires_at,
            max_uses: invite.max_uses,
            used_count: invite.used_count,
            revoked: invite.revoked,
        })
    }

    /// Deletes an invite outright.
    ///
    /// Revoking and deleting are different acts: revoking stops a code working the moment you
    /// realise it has escaped, and keeps the record. Deleting is the tidying-up afterwards, once a
    /// code is spent or dead and is only taking up room on screen.
    async fn delete_invite(&self, ctx: &Context<'_>, code: String) -> async_graphql::Result<bool> {
        require_admin(ctx)?;
        let db = ctx.data::<Db>()?;
        Ok(db.delete_invite(code.trim())?)
    }

    async fn revoke_invite(&self, ctx: &Context<'_>, code: String) -> async_graphql::Result<bool> {
        require_admin(ctx)?;
        ctx.data::<Db>()?.revoke_invite(&code).map_err(Into::into)
    }
}

/// An avatar has to be a URL this app will actually load, and nothing else.
///
/// `javascript:` and `data:` are the reason this is an allowlist rather than a length check: an
/// avatar is rendered in someone else's client, and a scheme is the cheapest thing to get wrong.
fn validated_avatar(raw: &str) -> async_graphql::Result<String> {
    let url = bounded(raw, 2048, "Avatar URL")?;
    if url.is_empty() || url.starts_with("https://") || url.starts_with("http://") {
        Ok(url)
    } else {
        Err("An avatar URL must start with http:// or https://".into())
    }
}

/// Tells the people allowed to know that `user` has started playing something else.
///
/// Called from `updateHandoff`, which is the only place a change of track exists. Two audiences,
/// deliberately separate: friends who are merely watching get a presence line, and listeners who
/// are following along get the position too, because their player has to act on it.
///
/// Every failure here is swallowed. This is a notification about somebody else's playback; it may
/// not be allowed to fail the handoff of the account that triggered it.
pub fn fan_out_presence(db: &Db, ws_hub: &crate::ws::WsHub, user: &str) {
    let Ok(Some(now)) = now_playing_of_quiet(db, user) else {
        return;
    };

    // The subject's own flag decides. A friend who has not opted in is not merely omitted from a
    // list here — nothing about them is sent at all.
    if db.profile(user).ok().flatten().is_some_and(|p| p.shows_now_playing()) {
        if let Ok(friends) = db.friends(user) {
            let audience: Vec<String> = friends.into_iter().map(|f| f.username).collect();
            if !audience.is_empty() {
                ws_hub.notify_users(
                    &audience,
                    "FRIEND_PRESENCE",
                    serde_json::json!({
                        "username": now.username,
                        "trackTitle": now.track_title,
                        "artistName": now.artist_name,
                        "albumName": now.album_name,
                        "artworkUrl": now.artwork_url,
                        "isPlaying": now.is_playing,
                        "updatedAt": now.updated_at,
                    }),
                );
            }
        }
    }

    // `listeners_of` re-checks the friendship, so someone who was unfriended mid-session stops
    // receiving frames whether or not their row was cleaned up.
    //
    // Sent one listener at a time rather than as a single fan-out, because two of these fields are
    // *pairwise*: whether the host is directly reachable, and the token to reach them with, are
    // facts about this listener and this host together. Broadcasting one payload would hand every
    // listener the first one's answer — including a LAN address and a bearer token that were never
    // theirs.
    if let Ok(listeners) = db.listeners_of(user) {
        for listener in listeners {
            let mut peer_lan_address = None;
            let mut peer_lan_token = None;
            if ws_hub.shares_network_with_user(user, &now.device_id, &listener) {
                if let Some(lan) = ws_hub.get_lan_address(user, &now.device_id) {
                    let listener_keys = published_keys(db, &listener);
                    peer_lan_token =
                        ws_hub.grant_p2p_token(user, &now.device_id, &listener, &listener_keys);
                    if peer_lan_token.is_some() {
                        peer_lan_address = Some(lan);
                    }
                }
            }
            ws_hub.notify_user(
                &listener,
                "LISTEN_ALONG",
                serde_json::json!({
                    "host": user,
                    "trackUri": now.track_uri,
                    "trackTitle": now.track_title,
                    "artistName": now.artist_name,
                    "albumName": now.album_name,
                    "artworkUrl": now.artwork_url,
                    "positionMs": now.position_ms,
                    "isPlaying": now.is_playing,
                    "updatedAt": now.updated_at,
                    "deviceId": now.device_id,
                    "contentHash": now.content_hash,
                    "peerLanAddress": peer_lan_address,
                    "peerLanToken": peer_lan_token,
                }),
            );
        }
    }
}

/// [`now_playing_of`] without the GraphQL error type, for the push path.
fn now_playing_of_quiet(db: &Db, username: &str) -> Result<Option<FriendNowPlaying>, ()> {
    now_playing_of(db, username).map_err(|_| ())
}
