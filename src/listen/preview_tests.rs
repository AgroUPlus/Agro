use super::preview::youtube_video_id;
use super::*;
use crate::db_playlist_items::NewPlaylistItem;
use crate::playlist_visibility::PlaylistVisibility;

fn params(id: Option<&str>, pl: Option<&str>) -> ListenParams {
    ListenParams {
        id: id.map(str::to_string),
        v: None,
        u: None,
        pl: pl.map(str::to_string),
        s: None,
        p: None,
    }
}

async fn page_for(state: AppState, params: ListenParams) -> String {
    let response = listen_handler(Query(params), State(state)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8(body.to_vec()).unwrap()
}

fn state_with_link(title: Option<&str>, subtitle: Option<&str>) -> AppState {
    let db = crate::db::Db::new(":memory:").unwrap();
    db.create_short_link(
        "trk1234",
        "https://music.youtube.com/watch?v=dQw4w9WgXcQ",
        None,
        None,
        None,
    )
    .unwrap();
    db.set_short_link_preview("trk1234", title, subtitle)
        .unwrap();
    crate::login::tests::test_state(db)
}

#[tokio::test]
async fn a_named_short_link_previews_its_track() {
    let state = state_with_link(Some("Paranoid Android"), Some("Radiohead"));
    let html = page_for(state, params(Some("trk1234"), None)).await;
    assert!(html.contains("<title>Paranoid Android · Radiohead</title>"));
    assert!(html.contains(r#"<meta property="og:title" content="Paranoid Android">"#));
    assert!(html.contains("Radiohead · Listen with Wanda"));
    assert!(html.contains("https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg"));
    assert!(!html.contains("__"), "a placeholder was left unfilled");
}

#[tokio::test]
async fn an_unnamed_short_link_gets_a_generic_preview_without_entities_in_its_title() {
    let html = page_for(state_with_link(None, None), params(Some("trk1234"), None)).await;
    assert!(html.contains("<title>Wanda · Shared track</title>"));
    assert!(!html.contains("&middot; Forwarding"));
}

/// §5a: a title is library metadata, never authored by this server.
#[tokio::test]
async fn a_hostile_title_is_escaped() {
    let state = state_with_link(Some("\"><script>alert(1)</script>"), None);
    let html = page_for(state, params(Some("trk1234"), None)).await;
    assert!(!html.contains("<script>alert(1)"));
    assert!(html.contains("&quot;&gt;&lt;script&gt;"));
}

fn state_with_playlist(visibility: PlaylistVisibility) -> (AppState, String) {
    let db = crate::db::Db::new(":memory:").unwrap();
    let playlist = db
        .create_playlist("owner", "Late Night Drive", None, visibility)
        .unwrap();
    let track = NewPlaylistItem {
        title: "Airbag".into(),
        artist: "Radiohead".into(),
        album: None,
        duration_ms: None,
        artwork_url: None,
        origin_uri: None,
    };
    db.add_playlist_item(&playlist.id, "owner", track).unwrap();
    (crate::login::tests::test_state(db), playlist.id)
}

#[tokio::test]
async fn a_public_playlist_previews_its_title_and_size() {
    let (state, id) = state_with_playlist(PlaylistVisibility::Public);
    let html = page_for(state, params(None, Some(&id))).await;
    assert!(html.contains("<title>Late Night Drive · 1 track</title>"));
    assert!(html.contains(r#"content="Late Night Drive""#));
    assert!(
        !html.contains("Radiohead"),
        "the tracks stay out of the page"
    );
}

/// A friends-only playlist's page must be the page an id that does not exist gets.
#[tokio::test]
async fn a_friends_playlist_says_nothing() {
    let (state, id) = state_with_playlist(PlaylistVisibility::Friends);
    let html = page_for(state.clone(), params(None, Some(&id))).await;
    assert!(!html.contains("Late Night Drive"));
    let unknown = "3f2b8c1e-9a4d-4e57-8b21-6d0f5c7a9e10";
    let unknown_html = page_for(state, params(None, Some(unknown))).await;
    assert_eq!(html.replace(&id, unknown), unknown_html);
}

#[test]
fn reads_video_ids_from_youtube_urls_only() {
    let id = Some("dQw4w9WgXcQ".to_string());
    assert_eq!(
        youtube_video_id("https://music.youtube.com/watch?v=dQw4w9WgXcQ&si=x"),
        id
    );
    assert_eq!(
        youtube_video_id("https://www.youtube.com/watch?feature=y&v=dQw4w9WgXcQ"),
        id
    );
    assert_eq!(youtube_video_id("https://youtu.be/dQw4w9WgXcQ?t=3"), id);
    assert_eq!(
        youtube_video_id("https://evil.example/watch?v=dQw4w9WgXcQ"),
        None
    );
    assert_eq!(youtube_video_id("https://youtube.com/watch?v=short"), None);
    assert_eq!(
        youtube_video_id("https://navidrome.example.com/share/abc"),
        None
    );
}
