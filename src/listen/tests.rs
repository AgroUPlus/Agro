use super::target::*;

#[test]
pub(super) fn accepts_a_well_formed_video_id() {
    assert!(is_video_id("dQw4w9WgXcQ"));
}

#[test]
pub(super) fn rejects_ids_that_are_not_ids() {
    assert!(!is_video_id("short"));
    assert!(!is_video_id("dQw4w9WgXcQ&x=1"));
    assert!(!is_video_id("../../etc/passwd"));
}

#[test]
pub(super) fn reads_the_host_of_an_https_url() {
    let parsed = url_host("https://Music.Example.com/rest?x=1").unwrap();
    assert_eq!(parsed.host, "music.example.com");
}

#[test]
pub(super) fn refuses_urls_that_disguise_their_host() {
    // The authority here is `evil.example`, however much it reads as youtube.com.
    assert!(url_host("https://music.youtube.com@evil.example/x").is_none());
    assert!(url_host("http://music.example.com/x").is_none());
    assert!(url_host("javascript:alert(1)").is_none());
    assert!(url_host("https://localhost/x").is_none());
}

#[test]
pub(super) fn resolves_short_link_uid_when_allowed() {
    let db = crate::db::Db::new(":memory:").unwrap();
    db.create_short_link(
        "testUid",
        "https://music.youtube.com/watch?v=dQw4w9WgXcQ",
        None,
        None,
        None,
    )
    .unwrap();
    let retrieved = db.get_short_link("testUid").unwrap();
    assert_eq!(
        retrieved.as_deref(),
        Some("https://music.youtube.com/watch?v=dQw4w9WgXcQ")
    );
}

#[test]
pub(super) fn expired_short_link_is_not_resolved() {
    let db = crate::db::Db::new(":memory:").unwrap();
    let past = chrono::Utc::now().timestamp() - 100;
    db.create_short_link(
        "expiredUid",
        "https://music.youtube.com/watch?v=dQw4w9WgXcQ",
        None,
        None,
        Some(past),
    )
    .unwrap();
    let retrieved = db.get_short_link("expiredUid").unwrap();
    assert_eq!(retrieved, None);
}
