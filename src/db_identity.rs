//! Accounts, roles and bearer tokens.
//!
//! A second `impl Db` block, the way [`crate::db_library`] is one, so `db.rs` does not grow another
//! few hundred lines of unrelated SQL.
//!
//! The model this replaces had exactly one kind of account. Every token could do everything, the
//! account passphrase *was* the bearer token, and both it and the device tokens were stored in the
//! clear. What lives here instead: one admin who owns the deployment, guests who can reach nothing
//! but their own data, and credentials that are hashed at rest.

use rusqlite::Result;

use crate::db::Db;

/// How much spool a new guest may occupy, in bytes. Ten mebibytes — enough to stage a queue, not
/// enough to be storage.
pub const DEFAULT_GUEST_QUOTA: i64 = 10 * 1024 * 1024;

/// How stale `last_used_at` is allowed to get before a request pays to refresh it.
///
/// This used to be written on *every* authenticated request, which put a database write — under
/// the global connection mutex — in the hot path of every read. The dashboard only shows this to
/// the hour, so an hour is all the precision it needs to be worth.
const LAST_USED_REFRESH_SECS: i64 = 3600;

/// How long a token may go unused before it stops working, from `AGRO_TOKEN_IDLE_DAYS`.
///
/// Idle expiry rather than absolute expiry, because the devices holding these tokens are a phone
/// and a terminal, not a browser tab: a credential that expires on a schedule makes a sync daemon
/// stop syncing at 3am for no reason a user can see. A credential that expires because nothing has
/// presented it in half a year is a device that is gone.
///
/// `0` disables it. The default is six months.
const DEFAULT_TOKEN_IDLE_DAYS: i64 = 180;

/// Whether a token should no longer be accepted, by absolute expiry or by disuse.
///
/// Unparseable timestamps are treated as *not* expired. A corrupt column must not silently log
/// everybody out, and the sweep will not remove the row either — the mismatch stays visible rather
/// than becoming a fleet-wide outage.
fn token_has_expired(
    expires_at: Option<&str>,
    last_used_at: Option<&str>,
    created_at: &str,
) -> bool {
    let now = chrono::Utc::now();

    if let Some(parsed) = expires_at.and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok()) {
        if parsed.with_timezone(&chrono::Utc) <= now {
            return true;
        }
    }

    let idle_days = token_idle_days();
    if idle_days == 0 {
        return false;
    }
    // `last_used_at` is NULL until the token is first presented, so a token that was minted and
    // never redeemed ages from `created_at` instead. This is the same `COALESCE` the sweep in
    // `sweep_expired_tokens` uses, and the two must agree: a token the sweep would delete must not
    // still authenticate in the window before the sweep runs.
    let Some(seen) = last_used_at
        .or(Some(created_at))
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
    else {
        return false;
    };
    (now - seen.with_timezone(&chrono::Utc)).num_days() >= idle_days
}

/// An optional absolute lifetime for newly minted tokens, from `AGRO_TOKEN_TTL_DAYS`.
///
/// Unset — the default — means no fixed expiry, because the devices holding these tokens are a
/// phone and a terminal and a sync daemon that stops working on a schedule is a support ticket.
/// Operators who want a hard ceiling can set one; idle expiry covers the common case on its own.
fn token_ttl_days() -> Option<i64> {
    std::env::var("AGRO_TOKEN_TTL_DAYS")
        .ok()
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|d| *d > 0)
}

fn token_idle_days() -> i64 {
    std::env::var("AGRO_TOKEN_IDLE_DAYS")
        .ok()
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|d| *d >= 0)
        .unwrap_or(DEFAULT_TOKEN_IDLE_DAYS)
}

/// What checking a second factor concluded.
///
/// Four outcomes rather than a bool, because the caller treats them differently: a replay must not
/// read as a wrong code (it means someone is reusing a code that was already spent, which is worth
/// logging as its own thing), and "not enrolled" must not read as "accepted".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TotpOutcome {
    Accepted,
    /// A recovery code was spent. The caller records this separately: it means the authenticator is
    /// gone, or someone else has the codes.
    AcceptedRecoveryCode,
    Rejected,
    /// Arithmetically correct, but for a time step already used.
    Replayed,
    NotEnrolled,
}

impl TotpOutcome {
    pub fn is_satisfied(self) -> bool {
        matches!(
            self,
            TotpOutcome::Accepted | TotpOutcome::AcceptedRecoveryCode
        )
    }
}

/// One row of the security log, as read back for display.
#[derive(Clone, Debug)]
pub struct SecurityEvent {
    pub id: i64,
    pub at: String,
    pub user_id: Option<String>,
    pub kind: String,
    pub client_ip: Option<String>,
    pub device_label: Option<String>,
    pub detail: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// Owns the deployment: the library, the settings, and every other account.
    Admin,
    /// A guest. Their own queue, their own friends, and nothing else.
    Member,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::Member => "member",
        }
    }

    /// Anything that is not exactly `admin` is a member.
    ///
    /// Deliberately not a round-trip of [`Self::as_str`]: a corrupt or unrecognised value in the
    /// column must fall to the *least* privileged reading, never to the most.
    pub fn parse(raw: &str) -> Self {
        if raw.trim().eq_ignore_ascii_case("admin") {
            Role::Admin
        } else {
            Role::Member
        }
    }

    pub fn is_admin(self) -> bool {
        matches!(self, Role::Admin)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccountState {
    /// Invited and registered, but not yet let in by the admin.
    Pending,
    Active,
    /// Was active; the admin took it away. Distinct from deletion so the history survives.
    Suspended,
}

impl AccountState {
    pub fn as_str(self) -> &'static str {
        match self {
            AccountState::Pending => "pending",
            AccountState::Active => "active",
            AccountState::Suspended => "suspended",
        }
    }

    /// Unrecognised values read as `Suspended`, for the same reason [`Role::parse`] falls to
    /// `Member`: an unreadable state must not be an open door.
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "active" => AccountState::Active,
            "pending" => AccountState::Pending,
            _ => AccountState::Suspended,
        }
    }

    pub fn is_active(self) -> bool {
        matches!(self, AccountState::Active)
    }
}

/// An account as the authorization layer needs to see it.
#[derive(Clone, Debug)]
pub struct Account {
    pub id: String,
    pub username: String,
    pub role: Role,
    pub state: AccountState,
    /// `0` means unlimited, which is what the admin has.
    pub quota_bytes: i64,
    pub can_archive: bool,
    pub show_now_playing: bool,
    pub show_stats: bool,
}

impl Account {
    pub fn is_admin(&self) -> bool {
        self.role.is_admin()
    }

    pub fn can_archive(&self) -> bool {
        self.role.is_admin() || self.can_archive
    }

    /// The admin is never capped. A quota on the person who owns the disk is theatre.
    pub fn effective_quota(&self) -> Option<i64> {
        if self.role.is_admin() || self.quota_bytes <= 0 {
            None
        } else {
            Some(self.quota_bytes)
        }
    }
}

const ACCOUNT_COLUMNS: &str =
    "id, username, role, state, quota_bytes, show_now_playing, show_stats, COALESCE(can_archive, 0)";

fn account_from_row(row: &rusqlite::Row<'_>) -> Result<Account> {
    Ok(Account {
        id: row.get(0)?,
        username: row.get(1)?,
        role: Role::parse(&row.get::<_, String>(2)?),
        state: AccountState::parse(&row.get::<_, String>(3)?),
        quota_bytes: row.get(4)?,
        show_now_playing: row.get::<_, i64>(5)? != 0,
        show_stats: row.get::<_, i64>(6)? != 0,
        can_archive: row.get::<_, i64>(7)? != 0,
    })
}

mod accounts;
mod events;
mod export;
mod federated;
mod settings;
#[cfg(test)]
mod tests;
mod tokens;
mod totp;
mod vault;
