//! Friendships, profiles, and the pointer that says whose playback you are following.
//!
//! Its own `impl Db` block for the same reason `db_identity` and `db_library` are: `db.rs` is
//! already 1300 lines of unrelated SQL.
//!
//! The rule the whole module exists to enforce: **a friendship is only a door, never a window.**
//! Being someone's friend does not by itself reveal anything. Each surface — now playing, stats —
//! has its own flag on the *subject's* account, and the flags default closed. Reading the graph
//! and reading what the graph gates are therefore two separate questions, and callers must ask
//! both.

/// The device id migration 36 files pre-registry keys under.
///
/// An account that has only ever had one device has a key but no device id to attribute it to —
/// it was published before device ids existed here. Sealing to it keeps that phone working until
/// it next signs in and claims its own entry.
pub const LEGACY_DEVICE_ID: &str = "legacy";

/// Where a pair of accounts stands.
///
/// Stored as text rather than an integer so a human reading the table with `sqlite3` can tell what
/// they are looking at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FriendState {
    /// A request has been sent and not yet answered.
    Pending,
    Accepted,
    /// One-directional and deliberately not symmetric: the blocker's row exists, the blocked
    /// account's does not, and nothing tells the blocked account which of the two happened.
    Blocked,
}

impl FriendState {
    pub fn as_str(self) -> &'static str {
        match self {
            FriendState::Pending => "pending",
            FriendState::Accepted => "accepted",
            FriendState::Blocked => "blocked",
        }
    }

    /// Unrecognised values read as `Blocked`, on the same principle as `Role::parse` and
    /// `AccountState::parse`: a row we cannot interpret must not become an open door.
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "accepted" => FriendState::Accepted,
            "pending" => FriendState::Pending,
            _ => FriendState::Blocked,
        }
    }
}

/// One account as another account is allowed to see it.
#[derive(Clone, Debug)]
pub struct Profile {
    pub username: String,
    pub display_name: Option<String>,
    pub bio: Option<String>,
    pub avatar_url: Option<String>,
    pub created_at: String,
    pub show_now_playing: bool,
    pub show_stats: bool,
    pub discoverable: bool,
    /// Whether accepted friends may see this account's library. Off until it is turned on.
    pub share_library: bool,
    /// Whether accepted friends may read this account's listening *history* — the activity feed
    /// and the circle recap. Separate from `show_stats` because an aggregate ("you played 40 hours
    /// of Aphex Twin") and a timeline ("you played this at 3am on Tuesday") are different
    /// disclosures, and someone may reasonably want the first without the second.
    pub show_activity: bool,
    /// Whether the account has gone quiet for the moment.
    ///
    /// Not a consent like the switches above but a temporary override of all of them: while this
    /// is on, every social surface is closed regardless of what the others say. Enforced in
    /// `visible_profile`, so no caller has to remember to ask.
    pub incognito: bool,
    /// The public identity key (e.g. X25519) used for end-to-end encrypted track drops and messages.
    pub public_key: Option<String>,
    /// Whether this account's plays count toward the fleet-wide Popular on Agro chart.
    ///
    /// Unlike the switches above, this defaults open — see migration 48: `popularity_counters`
    /// never stores an account id, so there is no identity here for a default to leak.
    pub popular_opt_in: bool,
}

/// One device's published identity key.
///
/// A device id is chosen by the client and is only ever meaningful within one account — see the
/// composite primary key in migration 36, and the same reasoning in migration 10.
#[derive(Clone, Debug)]
pub struct DeviceKey {
    pub device_id: String,
    pub public_key: String,
}

/// An edge as the *viewer* experiences it: who, and which way the unanswered request points.
#[derive(Clone, Debug)]
pub struct FriendEdge {
    pub profile: Profile,
    pub state: FriendState,
    /// True when the viewer sent the request, false when they received it. Meaningless unless
    /// `state` is `Pending`, and the caller is expected to know that.
    pub outgoing: bool,
}

/// How many columns [`PROFILE_COLUMNS`] selects, so anything appended after them can be indexed
/// relative to it rather than by a number that has to be remembered.
const PROFILE_COLUMN_COUNT: usize = 13;

const PROFILE_COLUMNS: &str =
    "username, display_name, bio, avatar_url, created_at, show_now_playing, show_stats, discoverable, share_library, show_activity, incognito, public_key, popular_opt_in";

fn profile_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Profile> {
    Ok(Profile {
        username: row.get(0)?,
        display_name: row.get(1)?,
        bio: row.get(2)?,
        avatar_url: row.get(3)?,
        created_at: row.get(4)?,
        show_now_playing: row.get::<_, i64>(5)? != 0,
        show_stats: row.get::<_, i64>(6)? != 0,
        discoverable: row.get::<_, i64>(7)? != 0,
        share_library: row.get::<_, i64>(8)? != 0,
        show_activity: row.get::<_, i64>(9)? != 0,
        incognito: row.get::<_, i64>(10)? != 0,
        public_key: row.get(11)?,
        popular_opt_in: row.get::<_, i64>(12)? != 0,
    })
}

impl Profile {
    /// Whether this account currently shows what it is playing.
    ///
    /// The standing consent *and* incognito, together, so that no caller can read the raw flag and
    /// forget the override. The first version of incognito checked only in `visible_profile`, and
    /// `friends_now_playing` — which never calls it — carried on broadcasting; the boundary test
    /// for it failed before this method existed.
    pub fn shows_now_playing(&self) -> bool {
        self.show_now_playing && !self.incognito
    }

    /// Whether aggregate listening statistics are open.
    pub fn shows_stats(&self) -> bool {
        self.show_stats && !self.incognito
    }

    /// Whether the activity timeline is open.
    pub fn shows_activity(&self) -> bool {
        self.show_activity && !self.incognito
    }

    /// Whether this account's plays should be counted into Popular on Agro.
    pub fn contributes_to_popular(&self) -> bool {
        self.popular_opt_in && !self.incognito
    }
}

/// Who someone is tuned in to, and since when.
#[derive(Clone, Debug)]
pub struct ListenAlong {
    pub listener: String,
    pub host: String,
    pub started_at: String,
}

/// A short-lived, single-use code that adds the account that minted it as a friend.
///
/// Not stored hashed, unlike a device token: it lives for minutes, is shown on screen on purpose,
/// and has to be matched by exact value from a QR scan. Hashing it would buy nothing that its
/// lifetime does not already buy.
#[derive(Clone, Debug)]
pub struct FriendCode {
    pub code: String,
    pub user_id: String,
    pub created_at: String,
    pub expires_at: String,
}

/// An invite code as the admin manages it.
#[derive(Clone, Debug)]
pub struct Invite {
    pub code: String,
    pub created_by: String,
    pub created_at: String,
    pub expires_at: Option<String>,
    pub max_uses: i64,
    pub used_count: i64,
    pub revoked: bool,
}

mod friends;
mod invites;
mod listen_along;
mod profiles;
#[cfg(test)]
mod tests;
