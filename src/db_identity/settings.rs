//! Per-account switches an admin or the owner can flip.

use super::*;
use crate::lock::LockExt;
use rusqlite::{params, Result};

impl Db {
    pub fn set_account_state(&self, username: &str, state: AccountState) -> Result<bool> {
        let conn = self.conn.lock_recover();
        let changed = conn.execute(
            "UPDATE users SET state = ?1 WHERE username = ?2 COLLATE NOCASE",
            params![state.as_str(), username.trim()],
        )?;
        Ok(changed > 0)
    }

    pub fn set_account_quota(&self, username: &str, quota_bytes: i64) -> Result<bool> {
        let conn = self.conn.lock_recover();
        let changed = conn.execute(
            "UPDATE users SET quota_bytes = ?1 WHERE username = ?2 COLLATE NOCASE",
            params![quota_bytes.max(0), username.trim()],
        )?;
        Ok(changed > 0)
    }

    pub fn set_can_archive(&self, username: &str, can_archive: bool) -> Result<bool> {
        let conn = self.conn.lock_recover();
        let changed = conn.execute(
            "UPDATE users SET can_archive = ?1 WHERE username = ?2 COLLATE NOCASE",
            params![if can_archive { 1 } else { 0 }, username.trim()],
        )?;
        Ok(changed > 0)
    }

    /// Whether accepted friends may browse this account's library.
    ///
    /// Its own setter rather than another parameter on `set_visibility`: sharing a music
    /// collection is a bigger decision than showing a now-playing line, and bundling it into the
    /// same call is how it ends up being turned on by a client that meant to set something else.
    pub fn set_share_library(&self, username: &str, share: bool) -> Result<bool> {
        let conn = self.conn.lock_recover();
        let changed = conn.execute(
            "UPDATE users SET share_library = ?1 WHERE username = ?2 COLLATE NOCASE",
            params![share as i64, username.trim()],
        )?;
        Ok(changed > 0)
    }

    /// Whether accepted friends may read this account's listening history.
    ///
    /// Its own setter for the same reason `set_share_library` is one: a history is a bigger
    /// disclosure than a now-playing line, and it should never be switched on as a side effect of
    /// a client sending a whole visibility struct back with one field changed.
    pub fn set_show_activity(&self, username: &str, show: bool) -> Result<bool> {
        let conn = self.conn.lock_recover();
        let changed = conn.execute(
            "UPDATE users SET show_activity = ?1 WHERE username = ?2 COLLATE NOCASE",
            params![show as i64, username.trim()],
        )?;
        Ok(changed > 0)
    }

    /// Turns the account quiet, or lets it speak again.
    ///
    /// Deliberately separate from [`Self::set_visibility`]: the switches there are standing
    /// consents the user set once, and incognito is a temporary override of all of them. Folding
    /// the two together would mean leaving incognito had to restore the other switches from
    /// somewhere, which is how a privacy setting silently comes back on.
    pub fn set_incognito(&self, username: &str, incognito: bool) -> Result<bool> {
        let conn = self.conn.lock_recover();
        let changed = conn.execute(
            "UPDATE users SET incognito = ?1 WHERE username = ?2 COLLATE NOCASE",
            params![incognito as i64, username.trim()],
        )?;
        Ok(changed > 0)
    }

    /// Whether this account's plays count toward the fleet-wide Popular on Agro chart.
    ///
    /// Its own setter for the same reason `set_share_library` and `set_show_activity` are: a
    /// client resending the whole visibility struct with one field changed must not silently
    /// re-enable a contribution decision it never touched.
    pub fn set_popular_opt_in(&self, username: &str, opt_in: bool) -> Result<bool> {
        let conn = self.conn.lock_recover();
        let changed = conn.execute(
            "UPDATE users SET popular_opt_in = ?1 WHERE username = ?2 COLLATE NOCASE",
            params![opt_in as i64, username.trim()],
        )?;
        Ok(changed > 0)
    }

    pub fn set_visibility(
        &self,
        username: &str,
        show_now_playing: bool,
        show_stats: bool,
    ) -> Result<bool> {
        let conn = self.conn.lock_recover();
        let changed = conn.execute(
            "UPDATE users SET show_now_playing = ?1, show_stats = ?2
              WHERE username = ?3 COLLATE NOCASE",
            params![show_now_playing as i64, show_stats as i64, username.trim()],
        )?;
        Ok(changed > 0)
    }
}
