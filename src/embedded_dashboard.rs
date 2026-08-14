use axum::{
    body::Body,
    http::{header, HeaderValue, StatusCode, Uri},
    response::{IntoResponse, Response},
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
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, HeaderValue::from_str(mime.as_ref()).unwrap())
                .body(Body::from(content.data))
                .unwrap()
        }
        None => {
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/html")
                .body(Body::from("<h1>Agro Web Dashboard</h1><p>Frontend assets building...</p>"))
                .unwrap()
        }
    }
}
