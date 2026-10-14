use super::*;
use std::path::{Path, PathBuf};

pub(super) fn filing<'a>(artist: &'a str, album: &'a str, title: &'a str) -> Filing<'a> {
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
pub(super) fn archive_hook_is_absent_unless_it_says_something() {
    std::env::remove_var("AGRO_ARCHIVE_HOOK");
    assert_eq!(
        Storage::from_env().archive_hook,
        None,
        "unset means no hook"
    );

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
pub(super) fn files_into_artist_album_track() {
    let path = relative_path(&filing("Nirvana", "Nevermind", "Come As You Are"));
    assert_eq!(
        path,
        PathBuf::from("Nirvana/Nevermind/01 - Come As You Are.flac")
    );
}

#[test]
pub(super) fn album_artist_wins_over_track_artist() {
    let path = relative_path(&Filing {
        album_artist: Some("Various Artists"),
        ..filing("Some Guest", "Comp", "Song")
    });
    assert!(path.starts_with("Various Artists"));
}

#[test]
pub(super) fn missing_tags_get_readable_fallbacks() {
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
pub(super) fn traversal_cannot_escape_the_root() {
    for hostile in [
        "../../etc/passwd",
        "..",
        ".",
        "/etc/passwd",
        "a/../../../b",
        "....//....//etc",
    ] {
        let path = relative_path(&filing(hostile, hostile, hostile));
        let resolved =
            resolve_within(Path::new("/mnt/music"), &path).expect("sanitised paths always resolve");
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
pub(super) fn resolve_within_rejects_what_it_is_given_directly() {
    assert!(resolve_within(Path::new("/mnt/music"), Path::new("/etc/passwd")).is_err());
    assert!(resolve_within(Path::new("/mnt/music"), Path::new("../etc")).is_err());
    assert!(resolve_within(Path::new("/mnt/music"), Path::new("ok/fine.flac")).is_ok());
}

#[test]
pub(super) fn separators_and_control_characters_are_stripped() {
    let path = relative_path(&filing("AC/DC", "Back\\In\0Black", "T:N:T"));
    assert_eq!(path.components().count(), 3, "still three segments");
    let text = path.to_string_lossy();
    assert!(!text.contains('\0') && !text.contains('\\'));
    assert!(text.starts_with("AC DC/"));
}

#[test]
pub(super) fn long_titles_are_capped_but_keep_their_extension() {
    let long = "x".repeat(400);
    let path = relative_path(&filing("A", "B", &long));
    let name = path.file_name().unwrap().to_string_lossy().to_string();
    assert!(name.ends_with(".flac"));
    assert!(name.len() < 200, "capped, got {}", name.len());
}

#[test]
pub(super) fn reserved_device_names_are_replaced() {
    let path = relative_path(&filing("CON", "NUL", "aux"));
    assert!(path.starts_with("Unknown Artist"));
    assert!(!path.to_string_lossy().contains("NUL"));
}

#[test]
pub(super) fn extension_cannot_smuggle_a_separator() {
    let path = relative_path(&Filing {
        extension: "../sh",
        ..filing("A", "B", "C")
    });
    assert!(path.to_string_lossy().ends_with(".sh"));
    assert_eq!(path.components().count(), 3);
}

#[test]
pub(super) fn trailing_dots_and_spaces_are_trimmed() {
    let path = relative_path(&filing("Artist. ", "Album ..", "Title."));
    let text = path.to_string_lossy();
    assert!(text.starts_with("Artist/"));
    assert!(text.contains("/Album/"));
}
