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

use rusqlite::{params, OptionalExtension, Result};

use crate::db::Db;

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
const PROFILE_COLUMN_COUNT: usize = 12;

const PROFILE_COLUMNS: &str =
    "username, display_name, bio, avatar_url, created_at, show_now_playing, show_stats, discoverable, share_library, show_activity, incognito, public_key";

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
}

impl Db {
    pub fn profile(&self, username: &str) -> Result<Option<Profile>> {
        let conn = self.conn.lock().unwrap();
        let profile = conn
            .query_row(
                &format!(
                    "SELECT {PROFILE_COLUMNS} FROM users
                      WHERE username = ?1 COLLATE NOCASE AND state = 'active'"
                ),
                params![username.trim()],
                profile_from_row,
            )
            .optional()?;
        Ok(profile)
    }

    pub fn update_profile(
        &self,
        username: &str,
        display_name: Option<&str>,
        bio: Option<&str>,
        avatar_url: Option<&str>,
    ) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        // `COALESCE(?, column)` so omitting a field leaves it alone rather than blanking it — the
        // clients send partial updates and a null must not be a deletion.
        let changed = conn.execute(
            "UPDATE users
                SET display_name = COALESCE(?1, display_name),
                    bio          = COALESCE(?2, bio),
                    avatar_url   = COALESCE(?3, avatar_url)
              WHERE username = ?4 COLLATE NOCASE",
            params![display_name, bio, avatar_url, username.trim()],
        )?;
        Ok(changed > 0)
    }

    pub fn set_public_key(&self, username: &str, public_key: Option<&str>) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "UPDATE users SET public_key = ?1 WHERE username = ?2 COLLATE NOCASE",
            params![public_key, username.trim()],
        )?;
        Ok(changed > 0)
    }

    pub fn set_discoverable(&self, username: &str, discoverable: bool) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "UPDATE users SET discoverable = ?1 WHERE username = ?2 COLLATE NOCASE",
            params![discoverable as i64, username.trim()],
        )?;
        Ok(changed > 0)
    }

    /// How `viewer` stands with `subject`, looking at both rows.
    ///
    /// A block is reported from whichever side holds it. The caller cannot be allowed to see a
    /// difference between "they blocked me" and "we are not friends", so both must arrive here as
    /// the same answer to the only question that matters: no.
    pub fn friend_state(&self, viewer: &str, subject: &str) -> Result<Option<FriendState>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT state FROM friendships
              WHERE (user_id = ?1 COLLATE NOCASE AND friend_id = ?2 COLLATE NOCASE)
                 OR (user_id = ?2 COLLATE NOCASE AND friend_id = ?1 COLLATE NOCASE)",
        )?;
        let states: Vec<FriendState> = stmt
            .query_map(params![viewer.trim(), subject.trim()], |row| {
                Ok(FriendState::parse(&row.get::<_, String>(0)?))
            })?
            .collect::<Result<Vec<_>>>()?;

        Ok(if states.contains(&FriendState::Blocked) {
            Some(FriendState::Blocked)
        } else if states.contains(&FriendState::Accepted) {
            Some(FriendState::Accepted)
        } else {
            states.first().copied()
        })
    }

    /// Whether these two can see each other at all. The precondition for every social read.
    pub fn are_friends(&self, a: &str, b: &str) -> Result<bool> {
        Ok(self.friend_state(a, b)? == Some(FriendState::Accepted))
    }

    /// Records a request from `from` to `to`.
    ///
    /// Answers `Ok(false)` when there is already an edge, so a resolver cannot use repeated calls
    /// to learn whether one exists. Accepting an outstanding request from the other side is the
    /// caller's job, not a side effect hidden in here.
    pub fn send_friend_request(&self, from: &str, to: &str) -> Result<bool> {
        if from.trim().eq_ignore_ascii_case(to.trim()) {
            return Ok(false);
        }
        if self.friend_state(from, to)?.is_some() {
            return Ok(false);
        }
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "INSERT OR IGNORE INTO friendships (user_id, friend_id, state, created_at)
             VALUES (?1, ?2, 'pending', ?3)",
            params![
                from.trim().to_lowercase(),
                to.trim().to_lowercase(),
                chrono::Utc::now().to_rfc3339()
            ],
        )?;
        Ok(changed > 0)
    }

    /// Accepts a request that `from` sent to `to`.
    ///
    /// Writes the mirror row as well, so `friends()` stays a single indexed lookup instead of a
    /// two-directional scan on every read. Both rows in one transaction: a half-accepted friendship
    /// is visible to exactly one of the two people, which is the worst possible outcome for a
    /// privacy boundary.
    pub fn accept_friend_request(&self, to: &str, from: &str) -> Result<bool> {
        let (to, from) = (to.trim().to_lowercase(), from.trim().to_lowercase());
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let promoted = tx.execute(
            "UPDATE friendships SET state = 'accepted'
              WHERE user_id = ?1 COLLATE NOCASE AND friend_id = ?2 COLLATE NOCASE
                AND state = 'pending'",
            params![from, to],
        )?;
        if promoted == 0 {
            return Ok(false);
        }
        tx.execute(
            "INSERT OR REPLACE INTO friendships (user_id, friend_id, state, created_at)
             VALUES (?1, ?2, 'accepted', ?3)",
            params![to, from, chrono::Utc::now().to_rfc3339()],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// Removes both rows, whatever state they were in. Declining and unfriending are the same
    /// operation on the data; only the word in front of the user differs.
    pub fn remove_friend(&self, a: &str, b: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let removed = conn.execute(
            "DELETE FROM friendships
              WHERE (user_id = ?1 COLLATE NOCASE AND friend_id = ?2 COLLATE NOCASE)
                 OR (user_id = ?2 COLLATE NOCASE AND friend_id = ?1 COLLATE NOCASE)",
            params![a.trim(), b.trim()],
        )?;
        Ok(removed > 0)
    }

    /// Blocks `subject` for `blocker`, clearing whatever was there first.
    pub fn block_user(&self, blocker: &str, subject: &str) -> Result<bool> {
        if blocker.trim().eq_ignore_ascii_case(subject.trim()) {
            return Ok(false);
        }
        self.remove_friend(blocker, subject)?;
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO friendships (user_id, friend_id, state, created_at)
             VALUES (?1, ?2, 'blocked', ?3)",
            params![
                blocker.trim().to_lowercase(),
                subject.trim().to_lowercase(),
                chrono::Utc::now().to_rfc3339()
            ],
        )?;
        Ok(true)
    }

    pub fn unblock_user(&self, blocker: &str, subject: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let removed = conn.execute(
            "DELETE FROM friendships
              WHERE user_id = ?1 COLLATE NOCASE AND friend_id = ?2 COLLATE NOCASE
                AND state = 'blocked'",
            params![blocker.trim(), subject.trim()],
        )?;
        Ok(removed > 0)
    }

    /// Accepted friends, as profiles. Suspended and deleted accounts drop out via the join.
    pub fn friends(&self, username: &str) -> Result<Vec<Profile>> {
        self.edges_where(
            username,
            "f.user_id = ?1 COLLATE NOCASE AND f.state = 'accepted'",
            "f.friend_id",
        )
        .map(|edges| edges.into_iter().map(|e| e.profile).collect())
    }

    /// Requests waiting on this account to answer.
    pub fn incoming_requests(&self, username: &str) -> Result<Vec<FriendEdge>> {
        self.edges_where(
            username,
            "f.friend_id = ?1 COLLATE NOCASE AND f.state = 'pending'",
            "f.user_id",
        )
    }

    /// Requests this account has sent and not had answered.
    pub fn outgoing_requests(&self, username: &str) -> Result<Vec<FriendEdge>> {
        let mut edges = self.edges_where(
            username,
            "f.user_id = ?1 COLLATE NOCASE AND f.state = 'pending'",
            "f.friend_id",
        )?;
        for edge in &mut edges {
            edge.outgoing = true;
        }
        Ok(edges)
    }

    /// The one query shape all three edge listings share.
    ///
    /// `other_column` names the end of the edge that is *not* the caller, and is a hardcoded
    /// column name from this module — never anything a request supplied.
    fn edges_where(
        &self,
        username: &str,
        predicate: &str,
        other_column: &str,
    ) -> Result<Vec<FriendEdge>> {
        let conn = self.conn.lock().unwrap();
        let sql = format!(
            "SELECT {columns}, f.state FROM friendships f
               JOIN users u ON u.username = {other_column} COLLATE NOCASE
              WHERE {predicate} AND u.state = 'active'
              ORDER BY u.username",
            columns = PROFILE_COLUMNS
                .split(", ")
                .map(|c| format!("u.{c}"))
                .collect::<Vec<_>>()
                .join(", "),
        );
        let mut stmt = conn.prepare(&sql)?;
        let edges = stmt
            .query_map(params![username.trim()], |row| {
                Ok(FriendEdge {
                    profile: profile_from_row(row)?,
                    // Derived, not written down: `f.state` sits immediately after the profile
                    // columns, and a hardcoded index silently reads the wrong column the next
                    // time one is added to PROFILE_COLUMNS.
                    state: FriendState::parse(&row.get::<_, String>(PROFILE_COLUMN_COUNT)?),
                    outgoing: false,
                })
            })?
            .collect::<Result<Vec<_>>>()?;
        Ok(edges)
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

impl Db {
    /// The public user directory.
    ///
    /// Three conditions, all of which must hold: the account is active, it has opted into being
    /// discoverable, and it is not the searcher themselves. Blocks are *not* filtered here — that
    /// would make an absence from the results a signal — they are filtered by the resolver, which
    /// removes them along with everything else it will not show.
    ///
    /// Prefix-anchored rather than `%term%`: a substring search over a public directory lets anyone
    /// enumerate the whole thing one letter at a time.
    pub fn search_users(&self, searcher: &str, query: &str, limit: i64) -> Result<Vec<Profile>> {
        let term = query.trim().to_lowercase();
        if term.is_empty() {
            return Ok(Vec::new());
        }
        let pattern = format!("{}%", term.replace('%', "").replace('_', ""));
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "SELECT {PROFILE_COLUMNS} FROM users
              WHERE state = 'active' AND discoverable = 1
                AND username <> ?1 COLLATE NOCASE
                AND (username LIKE ?2 COLLATE NOCASE OR display_name LIKE ?2 COLLATE NOCASE)
              ORDER BY username
              LIMIT ?3"
        ))?;
        let found = stmt
            .query_map(params![searcher.trim(), pattern, limit.clamp(1, 20)], profile_from_row)?
            .collect::<Result<Vec<_>>>()?;
        Ok(found)
    }

    /// Tunes `listener` in to `host`, replacing whatever they were following.
    pub fn set_listen_along(&self, listener: &str, host: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO listen_along (listener_id, host_id, started_at)
             VALUES (?1, ?2, ?3)",
            params![
                listener.trim().to_lowercase(),
                host.trim().to_lowercase(),
                chrono::Utc::now().to_rfc3339()
            ],
        )?;
        Ok(())
    }

    pub fn clear_listen_along(&self, listener: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let removed = conn.execute(
            "DELETE FROM listen_along WHERE listener_id = ?1 COLLATE NOCASE",
            params![listener.trim()],
        )?;
        Ok(removed > 0)
    }

    pub fn listen_along_of(&self, listener: &str) -> Result<Option<ListenAlong>> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                "SELECT listener_id, host_id, started_at FROM listen_along
                  WHERE listener_id = ?1 COLLATE NOCASE",
                params![listener.trim()],
                |row| {
                    Ok(ListenAlong {
                        listener: row.get(0)?,
                        host: row.get(1)?,
                        started_at: row.get(2)?,
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    /// Everyone currently following `host`. This is the fan-out list for a `LISTEN_ALONG` push.
    ///
    /// Re-checks the friendship on the way out, so a listener who was removed as a friend stops
    /// receiving frames without anything having to remember to clean up their row.
    pub fn listeners_of(&self, host: &str) -> Result<Vec<String>> {
        // Collected and the lock released before the friendship checks below, each of which takes
        // the same mutex.
        let listeners: Vec<String> = {
            let conn = self.conn.lock().unwrap();
            let mut stmt = conn.prepare(
                "SELECT listener_id FROM listen_along WHERE host_id = ?1 COLLATE NOCASE",
            )?;
            let rows = stmt
                .query_map(params![host.trim()], |row| row.get(0))?
                .collect::<Result<Vec<_>>>()?;
            rows
        };
        let mut allowed = Vec::new();
        for listener in listeners {
            if self.are_friends(host, &listener)? {
                allowed.push(listener);
            }
        }
        Ok(allowed)
    }

    /// Mints a short-lived code for adding a friend in person.
    ///
    /// Deliberately not an [`Invite`]. Those create *accounts*, are minted by administrators and
    /// last for hours; this is minted by any account for itself, redeems into a friend edge and
    /// nothing else, and expires in minutes — a code photographed off someone's screen has to stop
    /// working before the person who photographed it gets home.
    ///
    /// Any earlier codes for the same account are deleted first, so only the one currently on
    /// screen works. A code left behind by a sheet that was closed is a code nobody is watching.
    pub fn create_friend_code(&self, user_id: &str, ttl_minutes: i64) -> Result<FriendCode> {
        let owner = user_id.trim().to_lowercase();
        let code = FriendCode {
            code: crate::credentials::mint_token().secret,
            user_id: owner.clone(),
            created_at: chrono::Utc::now().to_rfc3339(),
            expires_at: (chrono::Utc::now() + chrono::Duration::minutes(ttl_minutes.max(1)))
                .to_rfc3339(),
        };
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM friend_codes WHERE user_id = ?1", params![owner])?;
        conn.execute(
            "INSERT INTO friend_codes (code, user_id, created_at, expires_at, used_at)
             VALUES (?1, ?2, ?3, ?4, NULL)",
            params![code.code, code.user_id, code.created_at, code.expires_at],
        )?;
        Ok(code)
    }

    /// Drops an account's outstanding code, for a sheet being closed or the app going away.
    pub fn revoke_friend_codes(&self, user_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM friend_codes WHERE user_id = ?1",
            params![user_id.trim().to_lowercase()],
        )?;
        Ok(())
    }

    /// Spends a friend code and answers with whose it was.
    ///
    /// Single-use, and the claim and the read are one transaction: two people scanning the same
    /// screen at the same moment must not both succeed. `None` covers every way a code can fail —
    /// unknown, expired, already spent — because telling them apart would let someone probe which
    /// codes have existed.
    pub fn redeem_friend_code(&self, code: &str) -> Result<Option<String>> {
        let now = chrono::Utc::now().to_rfc3339();
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let claimed = tx.execute(
            "UPDATE friend_codes SET used_at = ?1
              WHERE code = ?2 AND used_at IS NULL AND expires_at > ?1",
            params![now, code.trim()],
        )?;
        let owner = if claimed > 0 {
            tx.query_row(
                "SELECT user_id FROM friend_codes WHERE code = ?1",
                params![code.trim()],
                |row| row.get::<_, String>(0),
            )
            .optional()?
        } else {
            None
        };
        tx.commit()?;
        Ok(owner)
    }

    /// Mints an invite code. Admin-only at the resolver; this does not check.
    pub fn create_invite(&self, created_by: &str, max_uses: i64, ttl_hours: Option<i64>) -> Result<Invite> {
        let invite = Invite {
            code: crate::credentials::mint_token().secret,
            created_by: created_by.trim().to_lowercase(),
            created_at: chrono::Utc::now().to_rfc3339(),
            expires_at: ttl_hours
                .map(|h| (chrono::Utc::now() + chrono::Duration::hours(h)).to_rfc3339()),
            max_uses: max_uses.max(1),
            used_count: 0,
            revoked: false,
        };
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO invites (code, created_by, created_at, expires_at, max_uses, used_count, revoked)
             VALUES (?1, ?2, ?3, ?4, ?5, 0, 0)",
            params![
                invite.code,
                invite.created_by,
                invite.created_at,
                invite.expires_at,
                invite.max_uses
            ],
        )?;
        Ok(invite)
    }

    /// Spends one use of a code, or reports that it cannot be spent.
    ///
    /// The check and the increment are one transaction: two people redeeming a single-use code at
    /// the same moment must not both get in.
    pub fn redeem_invite(&self, code: &str) -> Result<bool> {
        let now = chrono::Utc::now().to_rfc3339();
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let spent = tx.execute(
            "UPDATE invites SET used_count = used_count + 1
              WHERE code = ?1 AND revoked = 0 AND used_count < max_uses
                AND (expires_at IS NULL OR expires_at > ?2)",
            params![code.trim(), now],
        )?;
        tx.commit()?;
        Ok(spent > 0)
    }

    /// Hands a use back, for a signup that redeemed a code and then could not finish.
    ///
    /// Never below zero: a refund that outran its redemption would make a code usable more often
    /// than the operator allowed, which is the one thing `max_uses` exists to prevent.
    pub fn refund_invite(&self, code: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "UPDATE invites SET used_count = used_count - 1
              WHERE code = ?1 AND used_count > 0",
            params![code.trim()],
        )?;
        Ok(changed > 0)
    }

    /// Removes an invite outright, so a spent or revoked one stops cluttering the list.
    ///
    /// Distinct from revoking: revoking stops a code working but keeps the record, which is what
    /// you want the moment you realise a code has escaped. Deleting is the tidying-up afterwards.
    pub fn delete_invite(&self, code: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let removed = conn.execute("DELETE FROM invites WHERE code = ?1", params![code.trim()])?;
        Ok(removed > 0)
    }

    pub fn list_invites(&self) -> Result<Vec<Invite>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT code, created_by, created_at, expires_at, max_uses, used_count, revoked
               FROM invites ORDER BY created_at DESC",
        )?;
        let invites = stmt
            .query_map([], |row| {
                Ok(Invite {
                    code: row.get(0)?,
                    created_by: row.get(1)?,
                    created_at: row.get(2)?,
                    expires_at: row.get(3)?,
                    max_uses: row.get(4)?,
                    used_count: row.get(5)?,
                    revoked: row.get::<_, i64>(6)? != 0,
                })
            })?
            .collect::<Result<Vec<_>>>()?;
        Ok(invites)
    }

    pub fn revoke_invite(&self, code: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "UPDATE invites SET revoked = 1 WHERE code = ?1",
            params![code.trim()],
        )?;
        Ok(changed > 0)
    }

    /// Accounts waiting to be let in.
    pub fn pending_accounts(&self) -> Result<Vec<Profile>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "SELECT {PROFILE_COLUMNS} FROM users WHERE state = 'pending' ORDER BY created_at"
        ))?;
        let pending = stmt
            .query_map([], profile_from_row)?
            .collect::<Result<Vec<_>>>()?;
        Ok(pending)
    }
}
