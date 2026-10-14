//! The friend graph, blocks and friend codes.

use super::*;
use crate::db::Db;
use crate::lock::LockExt;
use rusqlite::{params, OptionalExtension, Result};

impl Db {
    /// How `viewer` stands with `subject`, looking at both rows.
    ///
    /// A block is reported from whichever side holds it. The caller cannot be allowed to see a
    /// difference between "they blocked me" and "we are not friends", so both must arrive here as
    /// the same answer to the only question that matters: no.
    pub fn friend_state(&self, viewer: &str, subject: &str) -> Result<Option<FriendState>> {
        let conn = self.read();
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
        let conn = self.conn.lock_recover();
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
        let mut conn = self.conn.lock_recover();
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
        let conn = self.conn.lock_recover();
        let removed = conn.execute(
            "DELETE FROM friendships
              WHERE (user_id = ?1 COLLATE NOCASE AND friend_id = ?2 COLLATE NOCASE)
                 OR (user_id = ?2 COLLATE NOCASE AND friend_id = ?1 COLLATE NOCASE)",
            params![a.trim(), b.trim()],
        )?;
        // Sealed presence outlives nothing. The feed already re-checks friendship before it sends,
        // so this is not what stops delivery — it is what stops the server keeping a copy of what
        // one of them was listening to on behalf of someone no longer entitled to it.
        //
        // Unlike a drop, which belongs to the person who received it and survives the friendship,
        // presence is a live status: there is no version of it that is still wanted afterwards.
        Db::forget_presence_between_in(&conn, a, b)?;
        Ok(removed > 0)
    }

    /// Blocks `subject` for `blocker`, clearing whatever was there first.
    pub fn block_user(&self, blocker: &str, subject: &str) -> Result<bool> {
        if blocker.trim().eq_ignore_ascii_case(subject.trim()) {
            return Ok(false);
        }
        self.remove_friend(blocker, subject)?;
        let conn = self.conn.lock_recover();
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
        let conn = self.conn.lock_recover();
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
    pub(super) fn edges_where(
        &self,
        username: &str,
        predicate: &str,
        other_column: &str,
    ) -> Result<Vec<FriendEdge>> {
        let conn = self.read();
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
        let conn = self.conn.lock_recover();
        conn.execute(
            "DELETE FROM friend_codes WHERE user_id = ?1",
            params![owner],
        )?;
        conn.execute(
            "INSERT INTO friend_codes (code, user_id, created_at, expires_at, used_at)
             VALUES (?1, ?2, ?3, ?4, NULL)",
            params![code.code, code.user_id, code.created_at, code.expires_at],
        )?;
        Ok(code)
    }

    /// Drops an account's outstanding code, for a sheet being closed or the app going away.
    pub fn revoke_friend_codes(&self, user_id: &str) -> Result<()> {
        let conn = self.conn.lock_recover();
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
        let mut conn = self.conn.lock_recover();
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
}
