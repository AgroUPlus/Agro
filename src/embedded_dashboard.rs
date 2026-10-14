use axum::{
    body::Body,
    http::{header, HeaderValue, StatusCode, Uri},
    response::IntoResponse,
};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "dashboard/dist/"]
pub struct DashboardAssets;

pub async fn static_dashboard_handler(uri: Uri) -> impl IntoResponse {
    let path = uri.path().trim_start_matches('/');

    let target = if path.is_empty() { "index.html" } else { path };

    match DashboardAssets::get(target).or_else(|| DashboardAssets::get("index.html")) {
        Some(content) => {
            let mime = mime_guess::from_path(target).first_or_octet_stream();
            // A MIME type is ASCII by construction; the fallback exists only to keep the type total.
            let content_type = HeaderValue::from_str(mime.as_ref())
                .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"));
            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, content_type)],
                Body::from(content.data),
            )
                .into_response()
        }
        None => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, HeaderValue::from_static("text/html"))],
            "<h1>Agro Web Dashboard</h1><p>Frontend assets building...</p>",
        )
            .into_response(),
    }
}
