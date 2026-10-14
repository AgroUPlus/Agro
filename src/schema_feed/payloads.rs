//! The feed payload types.

use crate::schema::StatEntry;
use async_graphql::SimpleObject;

#[derive(SimpleObject, Clone)]
pub struct FeedItemPayload {
    pub username: String,
    /// RFC3339, from the play that caused it — not the time this was read.
    pub at: String,
    /// `MILESTONE`, `ON_REPEAT` or `NEW_FAVOURITE`. A discriminator rather than a union, so a
    /// client that meets a kind it does not know can still render the summary line.
    pub kind: String,
    /// A ready-made sentence. Sent so that every client says the same thing rather than each
    /// reimplementing the phrasing and disagreeing about it.
    pub summary: String,
    pub artist: String,
    pub title: Option<String>,
    /// Plays for a milestone or a repeat; distinct tracks for a new favourite.
    pub count: i64,
}

#[derive(SimpleObject, Clone)]
pub struct AnthemPayload {
    pub title: String,
    pub artist: String,
    /// Plays across the whole circle.
    pub plays: i64,
    /// Who played it, and how often each. The interesting half of an anthem.
    pub by_member: Vec<StatEntry>,
}

#[derive(SimpleObject, Clone)]
pub struct TrendsetterPayload {
    pub username: String,
    /// How many of the circle's top tracks this account played before anybody else did.
    pub firsts: i64,
    /// A few of them, so the claim can be checked rather than merely asserted.
    pub examples: Vec<String>,
}

/// One cell of the taste matrix. Undirected — `a` and `b` are sorted, and each pair appears once.
#[derive(SimpleObject, Clone)]
pub struct TasteMatrixEntry {
    pub a: String,
    pub b: String,
    pub score: i64,
}

#[derive(SimpleObject, Clone)]
pub struct CircleRecapPayload {
    /// Echoed back, because a client that asked for `MONTH` and a server that defaulted to `WEEK`
    /// would otherwise disagree silently.
    pub period: String,
    /// Who is actually in it. Friends who have their statistics closed are absent, not listed
    /// empty — being in someone's recap is a thing you opt into.
    pub members: Vec<String>,
    pub anthem: Option<AnthemPayload>,
    pub top_tracks: Vec<StatEntry>,
    pub top_artists: Vec<StatEntry>,
    pub trendsetter: Option<TrendsetterPayload>,
    pub matrix: Vec<TasteMatrixEntry>,
}
