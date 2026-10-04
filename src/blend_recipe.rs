//! A blend's recipe: how far back it reads, how often it is rewritten, and how big and how mixed
//! it is. Plain values, so `db_blend` and the API share one definition of each.

/// How far back a member's listening is read.
#[derive(async_graphql::Enum, Copy, Clone, Debug, PartialEq, Eq)]
pub enum BlendWindow {
    FourWeeks,
    SixMonths,
    AllTime,
}

/// How often it is written again on its own.
#[derive(async_graphql::Enum, Copy, Clone, Debug, PartialEq, Eq)]
pub enum BlendRefresh {
    Daily,
    Weekly,
    /// Only when someone joins or leaves, or the recipe changes.
    Frozen,
}

/// How many of each member's most-played tracks are considered. Far more than any blend holds, so
/// discovery has depth, and bounded, so a decade of history costs the same as a month.
pub(crate) const TASTE_DEPTH: i64 = 200;
pub(crate) const DAY: i64 = 86_400;

#[derive(Clone, Debug)]
pub struct Blend {
    pub playlist_id: String,
    pub size: i64,
    pub mix: i64,
    pub window: BlendWindow,
    pub refresh: BlendRefresh,
    pub refreshed_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BlendMember {
    pub username: String,
    pub joined: bool,
}

impl BlendWindow {
    pub(crate) fn stored(self) -> &'static str {
        match self {
            Self::FourWeeks => "4w",
            Self::SixMonths => "6m",
            Self::AllTime => "all",
        }
    }
    pub(crate) fn from_stored(raw: &str) -> Self {
        match raw {
            "4w" => Self::FourWeeks,
            "all" => Self::AllTime,
            _ => Self::SixMonths,
        }
    }
    pub(crate) fn days(self) -> Option<i64> {
        match self {
            Self::FourWeeks => Some(28),
            Self::SixMonths => Some(182),
            Self::AllTime => None,
        }
    }
}

impl BlendRefresh {
    pub(crate) fn stored(self) -> &'static str {
        match self {
            Self::Daily => "daily",
            Self::Weekly => "weekly",
            Self::Frozen => "frozen",
        }
    }
    pub(crate) fn from_stored(raw: &str) -> Self {
        match raw {
            "daily" => Self::Daily,
            "frozen" => Self::Frozen,
            _ => Self::Weekly,
        }
    }
    fn every_secs(self) -> Option<i64> {
        match self {
            Self::Daily => Some(DAY),
            Self::Weekly => Some(7 * DAY),
            Self::Frozen => None,
        }
    }
}

impl Blend {
    /// Whether it should be written again before it is read. Never written yet always is.
    pub fn is_due(&self, now: i64) -> bool {
        let Some(at) = self
            .refreshed_at
            .as_deref()
            .and_then(crate::stats::parse_time)
        else {
            return true;
        };
        self.refresh
            .every_secs()
            .is_some_and(|every| now - at >= every)
    }

    /// When it will next be written on its own, Unix seconds. `None` when frozen.
    pub fn next_refresh_at(&self) -> Option<i64> {
        let at = self
            .refreshed_at
            .as_deref()
            .and_then(crate::stats::parse_time)?;
        Some(at + self.refresh.every_secs()?)
    }
}

/// The recipe a creator chose. Validated by the API before it gets here.
#[derive(Clone, Copy, Debug)]
pub struct BlendSettings {
    pub size: i64,
    pub mix: i64,
    pub window: BlendWindow,
    pub refresh: BlendRefresh,
}
