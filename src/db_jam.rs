//! Jam sessions: one queue, several people, and a rule for whose turn is next.
//!
//! Deliberately not listen-along. That mirrors one person — there is a host and everyone else
//! follows. A jam has no source: every member adds to the same queue, and in `democracy` mode the
//! order is decided by votes rather than by who typed fastest.
//!
//! The host is whoever created it. That is the only asymmetry: they set the mode, can drop a track
//! somebody else added, and can end the session. Everything else is equal between members.

/// How the next track is chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JamMode {
    /// Anyone adds; it plays in the order it was added. A shared queue, no ceremony.
    Open,
    /// Anyone adds; the most-voted track plays next. Ties fall back to whoever added first, so the
    /// order is always total and never arbitrary.
    Democracy,
}

impl JamMode {
    pub fn as_str(self) -> &'static str {
        match self {
            JamMode::Open => "open",
            JamMode::Democracy => "democracy",
        }
    }

    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "open" => JamMode::Open,
            // Unrecognised means the default, and the default is the one that needs agreement.
            _ => JamMode::Democracy,
        }
    }
}

/// Where a track has got to.
///
/// The middle state is the point. A `played` flag could say "done" but not "the room has not agreed
/// to this yet", so in democracy mode votes had nowhere to act except on the *order* of a queue
/// everything had already entered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JamTrackState {
    /// Suggested, waiting on the room.
    Proposed,
    /// Accepted, waiting its turn.
    Queued,
    Played,
}

impl JamTrackState {
    pub fn as_str(self) -> &'static str {
        match self {
            JamTrackState::Proposed => "proposed",
            JamTrackState::Queued => "queued",
            JamTrackState::Played => "played",
        }
    }
}

/// What the room is hearing, and since when.
#[derive(Clone, Debug)]
pub struct JamNowPlaying {
    pub track_id: String,
    pub title: String,
    pub artist: String,
    /// The member who queued it, and so the one who can hand the file over.
    pub added_by: String,
    /// Which of their devices holds it. `None` for anything queued from a streaming source.
    pub added_by_device: Option<String>,
    /// SHA-256 of the bytes, when the queueing device knew them.
    pub content_hash: Option<String>,
    pub artwork_url: Option<String>,
    pub duration_ms: i64,
    /// A stream with no end. It plays until the room skips it — see `jam_clock`.
    pub is_live: bool,
    pub started_at: String,
    /// How far in the room is, derived from `started_at` rather than reported by anyone.
    pub position_ms: i64,
}

#[derive(Clone, Debug)]
pub struct Jam {
    pub id: String,
    pub code: String,
    pub host: String,
    pub mode: JamMode,
    pub created_at: String,
    /// The track the whole room is on, decided here and pushed out.
    pub now_playing_id: Option<String>,
    /// When that track started, which is what makes this server the clock.
    pub started_at: Option<String>,
    pub visibility: JamVisibility,
}

/// Who can find a jam without being handed its code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JamVisibility {
    /// The code is the only way in. The default, because a room is private until said otherwise.
    Code,
    /// Accepted friends can see it and join it. Never the whole instance — an open jam is open to
    /// people you have already agreed to, not to strangers who happen to be on the server.
    Friends,
}

impl JamVisibility {
    pub fn as_str(self) -> &'static str {
        match self {
            JamVisibility::Code => "code",
            JamVisibility::Friends => "friends",
        }
    }

    pub fn parse(raw: &str) -> Self {
        if raw.trim().eq_ignore_ascii_case("friends") {
            JamVisibility::Friends
        } else {
            JamVisibility::Code
        }
    }
}

#[derive(Clone, Debug)]
pub struct JamTrack {
    pub id: String,
    pub added_by: String,
    pub track_uri: String,
    pub title: String,
    pub artist: String,
    pub artwork_url: Option<String>,
    pub added_at: String,
    pub duration_ms: i64,
    /// Approvals so far. Only meaningful on a proposal — the queue is not sorted by these.
    pub approvals: i64,
    /// Whether the account asking has already approved it.
    pub approved: bool,
    /// How many more approvals it needs before it joins the queue.
    pub still_needed: i64,
}

const JAM_COLUMNS: &str =
    "id, code, host, mode, created_at, now_playing_id, started_at, visibility";

fn jam_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Jam> {
    Ok(Jam {
        id: row.get(0)?,
        code: row.get(1)?,
        host: row.get(2)?,
        mode: JamMode::parse(&row.get::<_, String>(3)?),
        created_at: row.get(4)?,
        now_playing_id: row.get(5)?,
        started_at: row.get(6)?,
        visibility: JamVisibility::parse(&row.get::<_, String>(7)?),
    })
}

/// A join code that survives being read out loud.
///
/// Letters and digits only — the token prefix this replaced was base64url, so codes arrived with
/// `-` and `_` in them, which is a poor thing to dictate over a room and worse to type. `0/O` and
/// `1/I` are left out for the same reason: they are alphanumeric but indistinguishable in most
/// fonts, and a code is useless if it cannot be copied by eye.
fn generate_jam_code() -> String {
    use rand::Rng;
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut rng = rand::thread_rng();
    (0..8)
        .map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char)
        .collect()
}

/// Milliseconds since an RFC3339 instant, never negative.
///
/// An unparseable or future timestamp reads as zero: starting a track from the beginning is a far
/// better failure than seeking to a nonsense offset.
fn elapsed_ms(started_at: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(started_at)
        .map(|start| {
            (chrono::Utc::now() - start.with_timezone(&chrono::Utc))
                .num_milliseconds()
                .max(0)
        })
        .unwrap_or(0)
}

mod lifecycle;
mod playback;
mod queue;
