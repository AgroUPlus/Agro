//! Reading tags and cover art out of audio files.

use crate::storage::{self};
use crate::AppState;
use std::path::Path;

#[derive(Default)]
pub(super) struct FileTags {
    pub(super) title: Option<String>,
    pub(super) artist: Option<String>,
    pub(super) album: Option<String>,
    pub(super) album_artist: Option<String>,
    pub(super) track_no: Option<u32>,
    pub(super) extension: String,
    /// The embedded front cover, as bytes and the extension to store them under.
    ///
    /// Read here rather than in a separate pass because the file is already open, already on the
    /// blocking pool, and about to be moved somewhere else.
    pub(super) cover: Option<(Vec<u8>, &'static str)>,
}

/// Reads tags off the received file. Blocking, so it runs on the blocking pool.
pub(super) async fn read_tags(path: &Path) -> FileTags {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        use lofty::file::TaggedFileExt;
        use lofty::tag::Accessor;

        let mut tags = FileTags {
            extension: "bin".to_string(),
            ..Default::default()
        };
        let Ok(tagged) = lofty::read_from_path(&path) else {
            return tags;
        };
        tags.extension = match tagged.file_type() {
            lofty::file::FileType::Flac => "flac",
            lofty::file::FileType::Mpeg => "mp3",
            lofty::file::FileType::Opus => "opus",
            lofty::file::FileType::Vorbis => "ogg",
            lofty::file::FileType::Mp4 => "m4a",
            lofty::file::FileType::Wav => "wav",
            lofty::file::FileType::Aac => "aac",
            _ => "bin",
        }
        .to_string();

        let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) else {
            return tags;
        };
        tags.title = tag.title().map(|t| t.to_string());
        tags.artist = tag.artist().map(|t| t.to_string());
        tags.album = tag.album().map(|t| t.to_string());
        tags.album_artist = tag
            .get_string(&lofty::tag::ItemKey::AlbumArtist)
            .map(|t| t.to_string());
        tags.track_no = tag.track();
        tags.cover = front_cover(tag);
        tags
    })
    .await
    .unwrap_or_default()
}

/// The album art embedded in a tag, if any, as bytes and the extension to file them under.
///
/// Prefers a picture explicitly marked as the front cover and falls back to the first one, because
/// plenty of files carry exactly one untyped picture and it is always the cover. Anything that is
/// not a JPEG or a PNG is ignored: those two cover essentially every real file, and a format the
/// browser cannot display is not artwork as far as this is concerned.
pub(super) fn front_cover(tag: &lofty::tag::Tag) -> Option<(Vec<u8>, &'static str)> {
    use lofty::picture::{MimeType, PictureType};

    let picture = tag
        .pictures()
        .iter()
        .find(|p| p.pic_type() == PictureType::CoverFront)
        .or_else(|| tag.pictures().first())?;

    let extension = match picture.mime_type() {
        Some(MimeType::Jpeg) => "jpg",
        Some(MimeType::Png) => "png",
        _ => return None,
    };
    // A tag can carry a picture of any size. Refusing the absurd ones keeps one bad file from
    // filling the covers directory.
    let data = picture.data();
    if data.is_empty() || data.len() > MAX_COVER_BYTES {
        return None;
    }
    Some((data.to_vec(), extension))
}

/// Ceiling on an embedded cover. Well above any real front cover, well below anything alarming.
pub(super) const MAX_COVER_BYTES: usize = 8 * 1024 * 1024;

/// Writes an album's cover to disk and records it, unless the album already has one.
///
/// Silent on failure and deliberately so: a cover is decoration. An unwritable covers directory
/// must not turn a successful upload into a failed one.
pub(super) async fn store_cover(
    state: &AppState,
    album_artist: &str,
    album: &str,
    cover: (Vec<u8>, &str),
) {
    let key = crate::db_library::album_key(album_artist, album);
    if state.db.has_cover(&key) {
        return;
    }
    let (bytes, extension) = cover;
    let path = state.storage.cover_file(&key, extension);
    if let Some(parent) = path.parent() {
        if tokio::fs::create_dir_all(parent).await.is_err() {
            return;
        }
    }
    if tokio::fs::write(&path, &bytes).await.is_err() {
        tracing::warn!("library: could not write cover for {album_artist} — {album}");
        return;
    }
    let _ = state.db.set_cover(&key, album_artist, album, extension);
}

/// Extracts artwork for every archived album that has none yet.
///
/// One file per album is read, not one per track: covers are an album-level thing and reading the
/// other eleven would be eleven decodes for the same JPEG. Returns how many were written.
pub async fn reindex_covers(state: &AppState) -> usize {
    let Some(root) = state.storage.library_root.clone() else {
        return 0;
    };
    let Ok(albums) = state.db.albums_for_cover_backfill() else {
        return 0;
    };

    let mut written = 0;
    for (album_artist, album, relative) in albums {
        if state
            .db
            .has_cover(&crate::db_library::album_key(&album_artist, &album))
        {
            continue;
        }
        let Ok(path) = storage::resolve_within(&root, std::path::Path::new(&relative)) else {
            continue;
        };
        let mut tags = read_tags(&path).await;
        let Some(cover) = tags.cover.take() else {
            continue;
        };
        store_cover(state, &album_artist, &album, cover).await;
        written += 1;
    }
    written
}
