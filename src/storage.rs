//! Where files live, and what they are allowed to be called.
//!
//! Two roots, configured by environment and both optional in different ways:
//!
//! - **`AGRO_LIBRARY_ROOT`** — the music library, e.g. `/mnt/music`. Unset means index-only mode:
//!   the server still records who holds what, but never keeps the bytes. On a host with a few GB
//!   of disk that is the honest default.
//! - **`AGRO_SPOOL_ROOT`** — staging for in-flight uploads and for files waiting to be collected
//!   by a peer. Size-capped and TTL'd, because this is the disk that runs out.
//!
//! And one optional hook, **`AGRO_ARCHIVE_HOOK`** — a shell command run after a file is filed.
//! The library is a plain directory and agro treats it as one; anything that keeps its own index
//! of that directory needs telling, and this is how, without agro knowing what that thing is. A
//! Nextcloud data directory wants `docker exec -u www-data nextcloud php occ files:scan --path=…`;
//! a plain folder of music wants nothing, which is the default.
//!
//! The path building here is the security-critical part of the feature. The endpoint this
//! replaces joined a caller-supplied filename straight onto a directory, so `../../` escaped it.
//! Every segment is sanitised, and the assembled path is then *checked* to still be inside its
//! root — belt and braces, because getting this wrong writes attacker-chosen files as the service
//! user.

use std::path::{Component, Path, PathBuf};

/// Longest a single path segment may be. Filesystems cap at 255 bytes; leaving room means a long
/// title cannot push the extension off the end.
const MAX_SEGMENT: usize = 120;

/// Windows reserves these regardless of extension. The library may well be read over SMB.
const RESERVED: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

#[derive(Clone, Debug)]
pub struct Storage {
    /// `None` in index-only mode.
    pub library_root: Option<PathBuf>,
    pub spool_root: PathBuf,
    pub spool_max_bytes: u64,
    pub spool_ttl_hours: i64,
    /// Shell command run after a successful archive. `None` — the default — runs nothing.
    pub archive_hook: Option<String>,
}

impl Storage {
    pub fn from_env() -> Self {
        Storage {
            library_root: std::env::var("AGRO_LIBRARY_ROOT")
                .ok()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
                .map(PathBuf::from),
            spool_root: std::env::var("AGRO_SPOOL_ROOT")
                .unwrap_or_else(|_| "./spool".to_string())
                .into(),
            spool_max_bytes: std::env::var("AGRO_SPOOL_MAX_BYTES")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(2 * 1024 * 1024 * 1024),
            spool_ttl_hours: std::env::var("AGRO_SPOOL_TTL_HOURS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(72),
            archive_hook: std::env::var("AGRO_ARCHIVE_HOOK")
                .ok()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty()),
        }
    }

    pub fn archives(&self) -> bool {
        self.library_root.is_some()
    }

    /// Where an upload's bytes accumulate. Named by upload id, never by anything a caller chose.
    pub fn part_file(&self, upload_id: &str) -> PathBuf {
        self.spool_root.join(format!("{upload_id}.part"))
    }

    /// Where a completed file waits for a peer to collect it. Named by content hash, which this
    /// server computed itself.
    pub fn spool_file(&self, content_hash: &str) -> PathBuf {
        self.spool_root.join(format!("{content_hash}.bin"))
    }
}

/// The tags a file's shelf position is derived from.
pub struct Filing<'a> {
    pub album_artist: Option<&'a str>,
    pub artist: &'a str,
    pub album: Option<&'a str>,
    pub title: &'a str,
    pub track_no: Option<u32>,
    /// Lowercased, without the dot. Taken from the *file*, not from a caller-supplied name.
    pub extension: &'a str,
}

/// `Album Artist/Album/01 - Title.flac`, every segment sanitised.
///
/// Relative on purpose — it is stored in `library_tracks.archived_path` and joined onto the root
/// at read time, so moving the library later does not invalidate the index.
pub fn relative_path(filing: &Filing<'_>) -> PathBuf {
    let artist = segment(filing.album_artist.unwrap_or(filing.artist), "Unknown Artist");
    let album = segment(filing.album.unwrap_or(""), "Unknown Album");
    let title = segment(filing.title, "Untitled");
    let extension = extension(filing.extension);

    let file = match filing.track_no {
        // Zero-padded so a plain alphabetical listing — which is what a file manager and most
        // scanners give you — is still in playing order.
        Some(n) if n > 0 => format!("{n:02} - {title}.{extension}"),
        _ => format!("{title}.{extension}"),
    };

    PathBuf::from(artist).join(album).join(file)
}

/// Makes one path segment safe, falling back to [`fallback`] when nothing usable survives.
///
/// Strips separators and control characters, refuses `.`/`..` and reserved device names, collapses
/// whitespace, and trims the trailing dots and spaces that Windows silently drops.
fn segment(value: &str, fallback: &str) -> String {
    let cleaned: String = value
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                ' '
            } else {
                c
            }
        })
        .collect();

    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed: String = collapsed.trim_end_matches(['.', ' ']).to_string();

    let truncated = if trimmed.chars().count() > MAX_SEGMENT {
        trimmed.chars().take(MAX_SEGMENT).collect::<String>()
    } else {
        trimmed
    };
    let truncated = truncated.trim_end_matches(['.', ' ']).to_string();

    let lowered = truncated.to_lowercase();
    if truncated.is_empty()
        || truncated == "."
        || truncated == ".."
        || RESERVED.contains(&lowered.as_str())
    {
        return fallback.to_string();
    }
    truncated
}

/// A file extension reduced to plain lowercase alphanumerics, so it cannot introduce a separator
/// or a second dot.
fn extension(value: &str) -> String {
    let cleaned: String = value
        .trim_start_matches('.')
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(8)
        .collect::<String>()
        .to_lowercase();
    if cleaned.is_empty() {
        "bin".to_string()
    } else {
        cleaned
    }
}

/// Joins [`relative`] onto [`root`] and proves the result is still underneath it.
///
/// [`relative_path`] already makes traversal impossible, so reaching the error here means
/// something upstream changed. It is checked anyway: this is the last line before the server
/// writes to a caller-influenced path as the service user, and the endpoint this replaces is a
/// standing demonstration of what skipping the check costs.
pub fn resolve_within(root: &Path, relative: &Path) -> Result<PathBuf, String> {
    if relative.is_absolute() {
        return Err("refusing an absolute path".to_string());
    }
    if relative
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(format!("refusing a path with traversal: {}", relative.display()));
    }
    Ok(root.join(relative))
}

/// Adds ` (2)`, ` (3)`… before the extension until the name is free.
///
/// Two different recordings really can file to the same shelf position — a self-titled track on a
/// compilation, say — and overwriting one with the other would lose a file the user still holds
/// the only copy of.
pub fn unique_path(candidate: PathBuf) -> PathBuf {
    if !candidate.exists() {
        return candidate;
    }
    let parent = candidate.parent().map(Path::to_path_buf).unwrap_or_default();
    let stem = candidate
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let ext = candidate
        .extension()
        .map(|s| format!(".{}", s.to_string_lossy()))
        .unwrap_or_default();

    for n in 2..1000 {
        let next = parent.join(format!("{stem} ({n}){ext}"));
        if !next.exists() {
            return next;
        }
    }
    // A thousand collisions is not a real library; keep the last rather than loop forever.
    parent.join(format!("{stem} ({}){ext}", uuid::Uuid::new_v4()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filing<'a>(artist: &'a str, album: &'a str, title: &'a str) -> Filing<'a> {
        Filing {
            album_artist: None,
            artist,
            album: Some(album),
            title,
            track_no: Some(1),
            extension: "flac",
        }
    }

    /// One test rather than three: the environment is process-global, so cases that set the same
    /// variable cannot run as separate tests without racing each other.
    #[test]
    fn archive_hook_is_absent_unless_it_says_something() {
        std::env::remove_var("AGRO_ARCHIVE_HOOK");
        assert_eq!(Storage::from_env().archive_hook, None, "unset means no hook");

        std::env::set_var("AGRO_ARCHIVE_HOOK", "   ");
        assert_eq!(
            Storage::from_env().archive_hook,
            None,
            "whitespace is not a command"
        );

        std::env::set_var("AGRO_ARCHIVE_HOOK", "  occ files:scan  ");
        assert_eq!(
            Storage::from_env().archive_hook.as_deref(),
            Some("occ files:scan")
        );
        std::env::remove_var("AGRO_ARCHIVE_HOOK");
    }

    #[test]
    fn files_into_artist_album_track() {
        let path = relative_path(&filing("Nirvana", "Nevermind", "Come As You Are"));
        assert_eq!(
            path,
            PathBuf::from("Nirvana/Nevermind/01 - Come As You Are.flac")
        );
    }

    #[test]
    fn album_artist_wins_over_track_artist() {
        let path = relative_path(&Filing {
            album_artist: Some("Various Artists"),
            ..filing("Some Guest", "Comp", "Song")
        });
        assert!(path.starts_with("Various Artists"));
    }

    #[test]
    fn missing_tags_get_readable_fallbacks() {
        let path = relative_path(&Filing {
            album_artist: None,
            artist: "",
            album: None,
            title: "",
            track_no: None,
            extension: "",
        });
        assert_eq!(
            path,
            PathBuf::from("Unknown Artist/Unknown Album/Untitled.bin")
        );
    }

    /// The bug in the endpoint this replaces.
    #[test]
    fn traversal_cannot_escape_the_root() {
        for hostile in [
            "../../etc/passwd",
            "..",
            ".",
            "/etc/passwd",
            "a/../../../b",
            "....//....//etc",
        ] {
            let path = relative_path(&filing(hostile, hostile, hostile));
            let resolved = resolve_within(Path::new("/mnt/music"), &path)
                .expect("sanitised paths always resolve");
            assert!(
                resolved.starts_with("/mnt/music"),
                "{hostile:?} escaped to {resolved:?}"
            );
            assert!(
                !resolved.components().any(|c| c.as_os_str() == ".."),
                "{hostile:?} kept a traversal component"
            );
        }
    }

    #[test]
    fn resolve_within_rejects_what_it_is_given_directly() {
        assert!(resolve_within(Path::new("/mnt/music"), Path::new("/etc/passwd")).is_err());
        assert!(resolve_within(Path::new("/mnt/music"), Path::new("../etc")).is_err());
        assert!(resolve_within(Path::new("/mnt/music"), Path::new("ok/fine.flac")).is_ok());
    }

    #[test]
    fn separators_and_control_characters_are_stripped() {
        let path = relative_path(&filing("AC/DC", "Back\\In\0Black", "T:N:T"));
        assert_eq!(path.components().count(), 3, "still three segments");
        let text = path.to_string_lossy();
        assert!(!text.contains('\0') && !text.contains('\\'));
        assert!(text.starts_with("AC DC/"));
    }

    #[test]
    fn long_titles_are_capped_but_keep_their_extension() {
        let long = "x".repeat(400);
        let path = relative_path(&filing("A", "B", &long));
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        assert!(name.ends_with(".flac"));
        assert!(name.len() < 200, "capped, got {}", name.len());
    }

    #[test]
    fn reserved_device_names_are_replaced() {
        let path = relative_path(&filing("CON", "NUL", "aux"));
        assert!(path.starts_with("Unknown Artist"));
        assert!(!path.to_string_lossy().contains("NUL"));
    }

    #[test]
    fn extension_cannot_smuggle_a_separator() {
        let path = relative_path(&Filing {
            extension: "../sh",
            ..filing("A", "B", "C")
        });
        assert!(path.to_string_lossy().ends_with(".sh"));
        assert_eq!(path.components().count(), 3);
    }

    #[test]
    fn trailing_dots_and_spaces_are_trimmed() {
        let path = relative_path(&filing("Artist. ", "Album ..", "Title."));
        let text = path.to_string_lossy();
        assert!(text.starts_with("Artist/"));
        assert!(text.contains("/Album/"));
    }
}
