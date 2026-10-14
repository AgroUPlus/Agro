use super::*;

#[test]
pub(super) fn test_url_patterns() {
    assert!(SPOTIFY_RE.is_match("https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M"));
    assert!(SPOTIFY_RE.is_match("https://open.spotify.com/album/4m2880jivSbbyEGAKfITCa"));
    assert!(SPOTIFY_RE.is_match("https://open.spotify.com/track/0VjIjW4GlUZAMYd2vXMi3b"));

    assert!(DEEZER_RE.is_match("https://www.deezer.com/playlist/3010368142"));
    assert!(DEEZER_RE.is_match("https://deezer.com/us/album/302127"));
    assert!(DEEZER_RE.is_match("https://deezer.com/track/3135556"));

    assert!(APPLE_MUSIC_RE.is_match("https://music.apple.com/us/album/discovery/697194953"));
    assert!(APPLE_MUSIC_RE.is_match(
        "https://music.apple.com/playlist/today-hits/pl.f4d106fed2bd41149aaacabb233eb5eb"
    ));

    assert!(YT_WATCH_RE.is_match("https://www.youtube.com/watch?v=dQw4w9WgXcQ"));
    assert!(YT_WATCH_RE.is_match("https://music.youtube.com/watch?v=dQw4w9WgXcQ"));
    assert!(YT_WATCH_RE.is_match("https://youtu.be/dQw4w9WgXcQ"));

    assert!(YT_PLAYLIST_RE
        .is_match("https://music.youtube.com/playlist?list=PL4fGSI1pDJn6jXS_PEO37J1NUEn3Z11dO"));
}

#[test]
pub(super) fn test_iso8601_duration_parsing() {
    assert_eq!(parse_iso8601_duration("PT3M45S"), Some(225000));
    assert_eq!(parse_iso8601_duration("PT4M"), Some(240000));
    assert_eq!(parse_iso8601_duration("PT30S"), Some(30000));
}
