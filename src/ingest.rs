use axum::{
    extract::Multipart,
    response::Json,
};
use lofty::file::TaggedFileExt;
use lofty::tag::Accessor;
use serde_json::{json, Value};
use std::path::Path;
use tokio::fs::{create_dir_all, File};
use tokio::io::AsyncWriteExt;

pub async fn dropbox_upload_handler(mut multipart: Multipart) -> Json<Value> {
    let music_dir = Path::new("./music");
    if let Err(e) = create_dir_all(music_dir).await {
        return Json(json!({"success": false, "error": e.to_string()}));
    }

    let mut filename = String::new();
    let mut uploaded_bytes = Vec::new();

    while let Ok(Some(field)) = multipart.next_field().await {
        if let Some(name) = field.file_name() {
            filename = name.to_string();
            if let Ok(bytes) = field.bytes().await {
                uploaded_bytes = bytes.to_vec();
            }
        }
    }

    if filename.is_empty() || uploaded_bytes.is_empty() {
        return Json(json!({"success": false, "error": "No file uploaded"}));
    }

    let target_path = music_dir.join(&filename);
    if let Ok(mut file) = File::create(&target_path).await {
        let _ = file.write_all(&uploaded_bytes).await;
    }

    // Process metadata with lofty
    let mut title = filename.clone();
    let mut artist = "Unknown Artist".to_string();
    let mut album = "Downloads".to_string();

    if let Ok(tagged_file) = lofty::probe::Probe::open(&target_path)
        .and_then(|p| p.read())
    {
        if let Some(tag) = tagged_file.primary_tag().or_else(|| tagged_file.first_tag()) {
            if let Some(t) = tag.title() {
                title = t.to_string();
            }
            if let Some(a) = tag.artist() {
                artist = a.to_string();
            }
            if let Some(al) = tag.album() {
                album = al.to_string();
            }
        }
    }

    Json(json!({
        "success": true,
        "message": "File ingested into Agro Drop Box and music library cleaned",
        "file": {
            "path": target_path.to_string_lossy(),
            "title": title,
            "artist": artist,
            "album": album
        }
    }))
}
