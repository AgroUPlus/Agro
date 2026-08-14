use axum::{
    extract::{Path, State},
    response::Html,
};
use crate::AppState;

pub async fn share_handler(
    Path(token): Path<String>,
    State(state): State<AppState>,
) -> Html<String> {
    if let Ok(Some(share)) = state.db.get_ephemeral_share(&token) {
        let html = format!(
            r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1.0" />
  <title>Agro Ephemeral Share - {title}</title>
  <style>
    body {{
      margin: 0; padding: 0;
      background: #090a0f;
      color: #f1f5f9;
      font-family: system-ui, -apple-system, sans-serif;
      display: flex; align-items: center; justify-content: center;
      min-height: 100vh;
    }}
    .card {{
      background: rgba(30, 41, 59, 0.7);
      backdrop-filter: blur(16px);
      border: 1px solid rgba(255, 255, 255, 0.1);
      border-radius: 24px;
      padding: 32px;
      width: 360px;
      text-align: center;
      box-shadow: 0 20px 40px rgba(0, 0, 0, 0.5);
    }}
    .cover {{
      width: 200px; height: 200px;
      border-radius: 16px;
      background: linear-gradient(135deg, #6366f1, #a855f7, #ec4899);
      margin: 0 auto 24px auto;
      display: flex; align-items: center; justify-content: center;
      font-size: 48px;
    }}
    h2 {{ margin: 0 0 8px 0; font-size: 22px; color: #fff; }}
    h3 {{ margin: 0 0 24px 0; font-size: 16px; color: #94a3b8; font-weight: 400; }}
    audio {{ width: 100%; margin-top: 16px; border-radius: 12px; }}
    .badge {{
      display: inline-block;
      padding: 4px 12px;
      background: rgba(99, 102, 241, 0.2);
      color: #818cf8;
      border-radius: 100px;
      font-size: 12px;
      margin-top: 20px;
    }}
  </style>
</head>
<body>
  <div class="card">
    <div class="cover">🎵</div>
    <h2>{title}</h2>
    <h3>{artist}</h3>
    <audio controls autoplay src="{audio_url}"></audio>
    <div class="badge">⌛ Shared via Agro • Expires in 24h</div>
  </div>
</body>
</html>"#,
            title = share.track_title,
            artist = share.artist_name,
            audio_url = share.audio_url
        );
        Html(html)
    } else {
        Html(r#"<!DOCTYPE html>
<html>
<head><title>Link Expired - Agro</title></head>
<body style="background:#090a0f;color:#fff;font-family:sans-serif;display:flex;align-items:center;justify-content:center;height:100vh;">
  <div style="text-align:center;">
    <h1>⏳ Shared Link Expired or Invalid</h1>
    <p style="color:#94a3b8;">This 24-hour ephemeral music link is no longer available.</p>
  </div>
</body>
</html>"#.to_string())
    }
}
