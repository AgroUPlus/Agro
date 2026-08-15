//! Deciding when two files are the same *recording*.
//!
//! This is the question behind "that device has a track you don't": a 128 kbps rip and a FLAC of
//! the same song are the same recording, and offering the user a copy of something they already
//! own is the failure mode to avoid. File-level identity — the SHA-256 in `library_tracks` — is a
//! separate question and answers a different one ("do I need to move these bytes").
//!
//! **This runs on the server, not on the clients.** Both clients already have their own
//! deduplicators, and if the two normalised a title even slightly differently the shared index
//! would end up holding two conventions and the diff would quietly produce nonsense. Clients
//! therefore send raw `artist` and `title`; the values matched on are computed here, in one place,
//! from whatever they sent.
//!
//! Ported from Wanda's `TrackDeduplicator`, deliberately including its conservatism: hiding a
//! track the user wanted is far worse than showing them one they already have.

use regex::Regex;
use std::sync::LazyLock;
use unicode_normalization::UnicodeNormalization;

/// Two transfers of one recording run to nearly the same length; different arrangements do not.
/// Three seconds absorbs encoder and tagging drift without merging distinct takes.
pub const DURATION_TOLERANCE_MS: i64 = 3_000;

/// Editorial noise describing the *release* rather than the performance. "Song" and
/// "Song (Remastered 2011)" are the same recording, so this goes before matching.
static NOISE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)
        \b(
            remaster(ed)?(\s+\d{4})? | \d{4}\s+remaster
          | official\s+(music\s+)?(video|audio)
          | lyrics?(\s+video)? | album\s+version | single\s+version | original\s+mix
          | explicit | clean | hd | hq | visualizer | mv
        )\b",
    )
    .expect("NOISE is a literal pattern")
});

/// Markers of a genuinely different performance. Compared as a set, so two tracks only merge when
/// they carry the *same* markers — folding a live take into the studio cut would silently hide a
/// version the user deliberately owns, which is the one mistake this must never make.
static VARIANT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)
        \b(
            live | acoustic | unplugged | remix | rmx | demo | instrumental | karaoke
          | reprise | edit | mix | version | cover | session | extended | club | dub
          | slowed | sped\s*up | orchestral | piano | deluxe | bonus
        )\b",
    )
    .expect("VARIANT is a literal pattern")
});

static FEATURED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(feat|ft|featuring|with)\b.*").expect("literal"));
static NON_ALPHANUMERIC: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[^\p{L}\p{N}\s]").expect("literal"));
static WHITESPACE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").expect("literal"));

/// Everything the index matches on except duration, which needs a tolerance rather than equality.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordingKey {
    pub artist: String,
    pub title: String,
    /// Sorted and joined, so it can live in a single SQLite column and compare as a string.
    pub variants: String,
}

pub fn recording_key(artist: &str, title: &str) -> RecordingKey {
    RecordingKey {
        artist: normalize_artist(artist),
        title: normalize_title(title),
        variants: variants_of(title),
    }
}

/// The performing artist, with featured-guest clauses removed.
///
/// A "feat." clause is genuine noise: backends disagree about whether it belongs in the artist
/// field, the title, or neither, so the same file arrives spelled three ways.
///
/// **Collaborators are deliberately *not* split off.** Wanda's deduplicator reads as though it
/// takes only the primary artist — it calls `substringBefore(" & ")` — but its `fold()` has
/// already stripped the punctuation by then, so those splits never fire and it keeps the whole
/// string. That accident is the safer behaviour and is kept on purpose: splitting would reduce
/// "Simon & Garfunkel" to "simon" and merge the duo's records into Paul Simon's solo ones. Two
/// devices holding the same file carry the same tags anyway, so there is very little to gain and
/// a silent over-merge to lose.
pub fn normalize_artist(artist: &str) -> String {
    let folded = fold(artist);
    let without_featured = FEATURED.replace_all(&folded, " ");
    WHITESPACE
        .replace_all(&without_featured, " ")
        .trim()
        .to_string()
}

/// Song title with featured-artist clauses, release noise and variant markers removed.
pub fn normalize_title(title: &str) -> String {
    let folded = fold(title);
    let step = FEATURED.replace_all(&folded, " ");
    let step = NOISE.replace_all(&step, " ");
    let step = VARIANT.replace_all(&step, " ");
    WHITESPACE.replace_all(&step, " ").trim().to_string()
}

/// The variant markers present in a title, sorted and deduplicated.
///
/// Read from the folded text *after* noise removal, so "(Album Version)" does not register as a
/// variant while "(Live)" does.
pub fn variants_of(title: &str) -> String {
    let folded = fold(title);
    let without_noise = NOISE.replace_all(&folded, " ");
    let mut found: Vec<String> = VARIANT
        .find_iter(&without_noise)
        .map(|m| WHITESPACE.replace_all(m.as_str(), " ").to_string())
        .collect();
    found.sort();
    found.dedup();
    found.join(",")
}

/// Lowercase, strip diacritics and punctuation.
///
/// Punctuation removal is what lets "(Live)", "- Live" and "[live]" all reduce to the same token.
/// Decomposing to NFD and dropping the combining marks is what makes "Beyoncé" and "Beyonce" the
/// same artist.
fn fold(value: &str) -> String {
    let decomposed: String = value
        .to_lowercase()
        .nfd()
        .filter(|c| !unicode_normalization::char::is_combining_mark(*c))
        .collect();
    let stripped = NON_ALPHANUMERIC.replace_all(&decomposed, " ");
    WHITESPACE.replace_all(&stripped, " ").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diacritics_and_punctuation_fold_away() {
        assert_eq!(normalize_artist("Beyoncé"), "beyonce");
        assert_eq!(normalize_artist("Sigur Rós"), "sigur ros");
        assert_eq!(normalize_title("Hello, World!"), "hello world");
    }

    #[test]
    fn featured_artists_are_dropped_from_both_fields() {
        assert_eq!(normalize_artist("Drake feat. Rihanna"), "drake");
        assert_eq!(normalize_artist("Drake ft Rihanna"), "drake");
        assert_eq!(normalize_title("Work (feat. Drake)"), "work");
    }

    /// Collaborators stay. See [`normalize_artist`] — splitting them off would merge a duo's
    /// records into one member's solo catalogue.
    #[test]
    fn collaborators_are_not_split_off() {
        assert_eq!(normalize_artist("Simon & Garfunkel"), "simon garfunkel");
        assert_ne!(
            normalize_artist("Simon & Garfunkel"),
            normalize_artist("Paul Simon")
        );
        assert_eq!(normalize_artist("Drake & Future"), "drake future");
    }

    #[test]
    fn release_noise_is_not_a_different_recording() {
        assert_eq!(normalize_title("Come As You Are"), "come as you are");
        assert_eq!(
            normalize_title("Come As You Are (Remastered 2011)"),
            "come as you are"
        );
        assert_eq!(
            normalize_title("Come As You Are - Official Music Video"),
            "come as you are"
        );
        assert_eq!(
            normalize_title("Come As You Are [Album Version]"),
            "come as you are"
        );
    }

    /// The failure this must never make: a live take is not the studio cut.
    #[test]
    fn performance_variants_are_kept_apart() {
        let studio = recording_key("Nirvana", "Come As You Are");
        let live = recording_key("Nirvana", "Come As You Are (Live)");
        let acoustic = recording_key("Nirvana", "Come As You Are - Acoustic");

        assert_eq!(studio.title, live.title, "the title itself still matches");
        assert_ne!(studio.variants, live.variants);
        assert_ne!(live.variants, acoustic.variants);
        assert_eq!(studio.variants, "");
        assert_eq!(live.variants, "live");
    }

    /// A remaster of a live take is still that live take.
    #[test]
    fn noise_and_variant_compose() {
        let a = recording_key("Nirvana", "Come As You Are (Live) [Remastered 2011]");
        let b = recording_key("Nirvana", "Come As You Are - Live");
        assert_eq!(a, b);
    }

    #[test]
    fn variants_are_order_independent() {
        let a = recording_key("X", "Song (Live Acoustic)");
        let b = recording_key("X", "Song (Acoustic Live)");
        assert_eq!(a.variants, b.variants);
        assert_eq!(a, b);
    }

    #[test]
    fn same_recording_from_two_backends_matches() {
        let navidrome = recording_key("Radiohead", "Karma Police");
        let youtube = recording_key("Radiohead", "Karma Police (Official Music Video)");
        assert_eq!(navidrome, youtube);
    }
}
