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

use crate::db::Db;
use crate::db_social::Profile;
use crate::schema::{caller, forbidden, normalise_username};
use async_graphql::Context;

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
pub(crate) fn require_visible(
    ctx: &Context<'_>,
    subject: &str,
    surface: Surface,
) -> async_graphql::Result<Profile> {
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

mod friends_query;

mod payloads;

mod presence;

mod profile_query;

pub use friends_query::SocialFriendsQuery;

pub use payloads::profile_payload;

use payloads::shared;

use payloads::validated_avatar;

pub use payloads::DeviceKeyPayload;

pub use payloads::FriendCodePayload;

pub use payloads::FriendEdgePayload;

pub use payloads::FriendNowPlaying;

pub use payloads::InvitePayload;

pub use payloads::ListenAlongPayload;

pub use payloads::ProfilePayload;

pub use payloads::TasteMatch;

pub use payloads::TotpConfirmedPayload;

pub use payloads::TotpEnrolmentPayload;

pub use payloads::TotpStatus;

use payloads::FRIEND_CODE_TTL_MINUTES;

use presence::attach_sealed_presence;

pub use presence::fan_out_presence;

use presence::live_now_playing;

use presence::now_playing_for_viewer;

pub(crate) use presence::published_keys;

pub use profile_query::SocialProfileQuery;

mod friends_mutation;
mod profile_mutation;
mod second_factor;
mod sharing;
pub use friends_mutation::SocialFriendsMutation;
pub use profile_mutation::SocialProfileMutation;
pub use second_factor::SocialSecondFactorMutation;
pub use sharing::SocialSharingMutation;
