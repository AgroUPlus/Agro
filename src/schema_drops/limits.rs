//! Size and rate limits for drops, with the reasoning for each.

/// The longest note that may ride along with a drop.
///
/// A comment, not an essay. Long enough for the reason you sent it, short enough that an inbox row
/// stays a row.
pub(super) const MAX_NOTE_LEN: usize = 500;

pub(super) const MAX_CIPHERTEXT_LEN: usize = 4096;

/// How many devices one note may be sealed to.
///
/// A sender seals once per key in the recipient's registry plus once to its own, so this is a
/// bound on somebody's device count, not on anything they type. Well clear of a person with a
/// phone, a tablet and a desktop, and low enough that a client claiming a thousand devices cannot
/// turn one drop into a megabyte of rows.
pub(super) const MAX_SEALED_COPIES: usize = 16;

/// The longest any piece of track description may be. Titles are attacker-controlled here in a way
/// they are not elsewhere: this is text one account writes into another account's inbox.
pub(super) const MAX_FIELD_LEN: usize = 512;

pub(super) const MAX_URL_LEN: usize = 2048;

/// Most drops one account may send another inside [`RATE_WINDOW_SECS`].
///
/// Generous for anybody using the feature as intended — handing over an album a track at a time is
/// still comfortably inside it — and low enough that an inbox cannot be buried. The limit is per
/// *pair*, not per sender, so one tiresome friend cannot exhaust the quota you have for everyone
/// else, and blocking remains the answer to somebody who keeps hitting it.
pub(super) const MAX_DROPS_PER_WINDOW: i64 = 20;

pub(super) const RATE_WINDOW_SECS: i64 = 3600;

/// How many drops may be asked for at once.
pub(super) const MAX_PAGE: i64 = 100;

/// Long enough for a flag, a family or a skin-tone modifier — all of which are several code
/// points — and far too short to be a message.
pub(super) const MAX_REACTION_CHARS: usize = 8;
