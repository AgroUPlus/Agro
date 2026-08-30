use axum::{
    body::Body,
    extract::{Extension, Request},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::Response,
};
use axum::response::IntoResponse;
use std::sync::Arc;
use crate::auth::AuthedUser;
use crate::db::Db;

pub async fn proxy_handler(
    Extension(user): Extension<AuthedUser>,
    Extension(db): Extension<Arc<Db>>,
    headers: HeaderMap,
    req: Request<Body>,
) -> Response {
    let saved_states = db.get_plugin_states().unwrap_or_default();
    if !saved_states.get("privacy-relay").copied().unwrap_or(true) {
        return (StatusCode::FORBIDDEN, "Privacy relay is disabled by the administrator").into_response();
    }

    let target_url = match headers.get("X-Agro-Proxy-Url") {
        Some(v) => match v.to_str() {
            Ok(s) => s,
            Err(_) => return (StatusCode::BAD_REQUEST, "Invalid X-Agro-Proxy-Url").into_response(),
        },
        None => return (StatusCode::BAD_REQUEST, "Missing X-Agro-Proxy-Url").into_response(),
    };

    let allowed_domains = ["archive.org", "lrclib.net", "nyaa.si"];
    let parsed_url = match reqwest::Url::parse(target_url) {
        Ok(url) => url,
        Err(_) => return (StatusCode::BAD_REQUEST, "Invalid URL").into_response(),
    };

    let host = parsed_url.host_str().unwrap_or("");
    if !allowed_domains.iter().any(|d| host.contains(d)) {
        return (StatusCode::FORBIDDEN, "Domain not allowed for proxying").into_response();
    }

    // Check cache (only for GET requests)
    let is_get = req.method() == axum::http::Method::GET;
    if is_get {
        if let Ok(Some((cached_headers_json, cached_body))) = db.get_cached_proxy(target_url) {
            let mut response_builder = axum::http::Response::builder().status(StatusCode::OK);
            if let Ok(headers_map) = serde_json::from_str::<std::collections::HashMap<String, String>>(&cached_headers_json) {
                for (k, v) in headers_map {
                    response_builder = response_builder.header(&k, &v);
                }
            }
            return response_builder.body(Body::from(cached_body)).unwrap_or_else(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Cache rebuild failed").into_response());
        }
    }

    let client = reqwest::Client::builder().build().unwrap();
    let method = req.method().clone();
    
    // Read the body fully (preventing media streams from being sent here anyway)
    let body_bytes = match axum::body::to_bytes(req.into_body(), 10 * 1024 * 1024).await {
        Ok(b) => b,
        Err(_) => return (StatusCode::PAYLOAD_TOO_LARGE, "Payload too large").into_response(),
    };

    let mut proxy_req = client.request(method.clone(), target_url)
        .body(body_bytes);
        
    for (k, v) in headers.iter() {
        if k != "host" && k != "x-agro-proxy-url" && k != "authorization" && k != "content-length" {
            proxy_req = proxy_req.header(k.clone(), v.clone());
        }
    }

    let res = match proxy_req.send().await {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_GATEWAY, format!("Proxy error: {}", e)).into_response(),
    };

    let status = res.status();
    let mut response_headers = std::collections::HashMap::new();
    let mut builder = axum::http::Response::builder().status(status);
    
    for (k, v) in res.headers().iter() {
        builder = builder.header(k, v);
        if let Ok(val_str) = v.to_str() {
            response_headers.insert(k.as_str().to_string(), val_str.to_string());
        }
    }

    let response_body = match res.bytes().await {
        Ok(b) => b,
        Err(_) => return (StatusCode::BAD_GATEWAY, "Failed to read response").into_response(),
    };
    
    if is_get && status.is_success() {
        if let Ok(headers_json) = serde_json::to_string(&response_headers) {
            let expires_at = chrono::Utc::now().timestamp() + (24 * 60 * 60);
            let _ = db.set_cached_proxy(target_url, &headers_json, &response_body, expires_at);
        }
    }

    builder.body(Body::from(response_body)).unwrap_or_else(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Response build failed").into_response())
}
