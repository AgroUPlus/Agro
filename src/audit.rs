//! The append-only record of security-relevant events.
//!
//! Everything else in this codebase decides whether an action is *allowed*. This module records
//! that it *happened*, which is a different job and the one that was missing: an account that has
//! been taken over raises questions — when did this start, from where, which device, what did it
//! touch — that authorization checks cannot answer after the fact.
//!
//! Three rules hold everywhere in here, and each exists because the obvious implementation gets it
//! wrong:
//!
//! - **Never record a credential.** Not the passphrase, not the token, not the TOTP code that was
//!   presented. An audit log that captures failed logins captures the near-misses of real
//!   passphrases, so a log readable by an admin would otherwise be a slow leak of everyone else's
//!   secrets. [`Event::detail`] is for facts about the attempt, never the attempt's contents.
//! - **Addresses are truncated.** A /24 or a /64 shows a pattern of attempts just as well as a full
//!   address, and is not a record of where a user was sitting. See [`truncate_ip`].
//! - **Writing is best-effort.** A failure to record must never turn a working request into an
//!   error. The alternative — refusing the action because the log write failed — turns a full disk
//!   into a total outage, which is a worse failure than a gap in the record.
//!
//! There is deliberately no update or delete helper. The only thing that removes rows is the
//! retention sweep, because a log with no bound is itself the privacy problem it was meant to
//! address.

use std::net::IpAddr;

/// What happened. The string form is what lands in the `kind` column, so these are stable.
///
/// An enum rather than free-form strings: the dashboard filters on these, and a typo'd kind is an
/// event that silently never shows up in the view meant to display it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    LoginSucceeded,
    LoginFailed,
    /// The passphrase was right but the second factor was missing or wrong.
    TotpFailed,
    /// A recovery code was spent. Worth its own kind: it means someone lost a device, or someone
    /// else is using the codes.
    RecoveryCodeUsed,
    TotpEnrolled,
    TotpDisabled,
    PassphraseChanged,
    TokenMinted,
    TokenRevoked,
    /// Every other token on the account was revoked at once.
    AllTokensRevoked,
    AccountCreated,
    AccountStateChanged,
    AccountDeleted,
    IdentityLinked,
    IdentityUnlinked,
}

impl Event {
    pub fn as_str(self) -> &'static str {
        match self {
            Event::LoginSucceeded => "login_succeeded",
            Event::LoginFailed => "login_failed",
            Event::TotpFailed => "totp_failed",
            Event::RecoveryCodeUsed => "recovery_code_used",
            Event::TotpEnrolled => "totp_enrolled",
            Event::TotpDisabled => "totp_disabled",
            Event::PassphraseChanged => "passphrase_changed",
            Event::TokenMinted => "token_minted",
            Event::TokenRevoked => "token_revoked",
            Event::AllTokensRevoked => "all_tokens_revoked",
            Event::AccountCreated => "account_created",
            Event::AccountStateChanged => "account_state_changed",
            Event::AccountDeleted => "account_deleted",
            Event::IdentityLinked => "identity_linked",
            Event::IdentityUnlinked => "identity_unlinked",
        }
    }
}

/// One recorded event, built by the caller and handed to `Db::record_event`.
///
/// A struct with a builder rather than a nine-argument function, because most call sites know only
/// two or three of these and positional `None`s are how the wrong value ends up in the wrong
/// column.
#[derive(Debug, Default)]
pub struct Record {
    pub user_id: Option<String>,
    pub client_ip: Option<String>,
    pub device_label: Option<String>,
    pub detail: Option<String>,
}

impl Record {
    pub fn new() -> Self {
        Self::default()
    }

    /// The account this concerns. `None` for a failed login against a username that does not exist
    /// — which is the event most worth keeping, and has no account to attach to.
    pub fn user(mut self, username: impl Into<String>) -> Self {
        self.user_id = Some(username.into());
        self
    }

    pub fn maybe_user(mut self, username: Option<String>) -> Self {
        self.user_id = username;
        self
    }

    /// Truncated on the way in, so an untruncated address cannot reach the table by a call site
    /// forgetting to do it.
    pub fn ip(mut self, raw: &str) -> Self {
        self.client_ip = truncate_ip(raw);
        self
    }

    pub fn device(mut self, label: impl Into<String>) -> Self {
        self.device_label = Some(label.into());
        self
    }

    /// A short fact about the event. **Never a credential**; see the module docs.
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// Reduces an address to the network it came from: a /24 for IPv4, a /64 for IPv6.
///
/// Enough to see that forty attempts came from one place, not enough to be a location history. An
/// address that does not parse is dropped rather than stored verbatim — a value that reached here
/// without being an address is not one this table should keep.
pub fn truncate_ip(raw: &str) -> Option<String> {
    match raw.trim().parse::<IpAddr>().ok()? {
        IpAddr::V4(v4) => {
            let [a, b, c, _] = v4.octets();
            Some(format!("{a}.{b}.{c}.0/24"))
        }
        IpAddr::V6(v6) => {
            let s = v6.segments();
            Some(format!(
                "{:x}:{:x}:{:x}:{:x}::/64",
                s[0], s[1], s[2], s[3]
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ipv4_address_keeps_only_its_network() {
        assert_eq!(truncate_ip("192.168.1.57").unwrap(), "192.168.1.0/24");
        assert_eq!(truncate_ip("  8.8.8.8  ").unwrap(), "8.8.8.0/24");
    }

    #[test]
    fn an_ipv6_address_keeps_only_its_prefix() {
        assert_eq!(
            truncate_ip("2001:db8:85a3:1234:5678:8a2e:370:7334").unwrap(),
            "2001:db8:85a3:1234::/64"
        );
    }

    /// The host part must be gone, not merely rewritten — this is the whole point of the function.
    #[test]
    fn the_host_part_is_not_recoverable() {
        let truncated = truncate_ip("203.0.113.199").unwrap();
        assert!(!truncated.contains("199"), "{truncated}");
        assert_eq!(truncated, truncate_ip("203.0.113.4").unwrap());
    }

    /// Anything that is not an address is dropped rather than stored as-is, so a header value
    /// cannot smuggle arbitrary text into the column.
    #[test]
    fn a_non_address_is_dropped() {
        assert_eq!(truncate_ip("not-an-ip"), None);
        assert_eq!(truncate_ip(""), None);
        assert_eq!(truncate_ip("<script>alert(1)</script>"), None);
    }

    #[test]
    fn event_kinds_are_distinct_and_stable() {
        let kinds = [
            Event::LoginSucceeded,
            Event::LoginFailed,
            Event::TotpFailed,
            Event::RecoveryCodeUsed,
            Event::TotpEnrolled,
            Event::TotpDisabled,
            Event::PassphraseChanged,
            Event::TokenMinted,
            Event::TokenRevoked,
            Event::AllTokensRevoked,
            Event::AccountCreated,
            Event::AccountStateChanged,
            Event::AccountDeleted,
            Event::IdentityLinked,
            Event::IdentityUnlinked,
        ]
        .map(Event::as_str);
        let mut seen: Vec<&str> = kinds.to_vec();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), kinds.len(), "two events share a kind string");
    }
}
