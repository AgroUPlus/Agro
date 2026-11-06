//! The link preview a chat draws under a shared link: Open Graph tags, and the page's own title.
//!
//! A chat that unfurls a link reads these instead of running the page, so without them every
//! share showed as "Wanda · Forwarding". Only what the link is already public about goes in:
//!
//! - the title and subtitle the sharer's app minted a short link with (`?id=` only — the other
//!   forms carry no metadata and gain none here), and
//! - a YouTube thumbnail, when the target is a YouTube video. Its id is already in the target the
//!   page forwards to, so the thumbnail tells a reader nothing the link does not.
//!
//! Nothing is fetched to build a preview: a page that went off to a third party every time a chat
//! unfurled it would tell that party who was sharing what.

use super::escape_html;
use super::target::{is_video_id, url_host};
use super::ListenParams;
use crate::db_short_link_preview::LinkPreview;
use crate::AppState;

/// What the page and its preview show. Plain text: every accessor's caller escapes it.
pub(crate) struct Preview {
    pub(crate) title: Option<String>,
    pub(crate) subtitle: Option<String>,
    pub(crate) image: Option<String>,
    /// What the link is of when nothing names it: "track", "playlist".
    pub(crate) noun: &'static str,
}

/// The preview for a track link that resolved to `target`.
pub(super) fn track(params: &ListenParams, state: &AppState, target: &str) -> Preview {
    let named = params.id.as_deref().and_then(|id| {
        state.db.short_link_preview(id).unwrap_or_else(|e| {
            tracing::warn!("listen: preview lookup failed: {e}");
            None
        })
    });
    let (title, subtitle) = match named {
        Some(LinkPreview { title, subtitle }) => (Some(title), subtitle),
        None => (None, None),
    };
    Preview {
        title,
        subtitle,
        image: youtube_video_id(target)
            .map(|id| format!("https://i.ytimg.com/vi/{id}/hqdefault.jpg")),
        noun: "track",
    }
}

impl Preview {
    /// The `<title>`: "Song · Artist", or a generic line when the link names nothing.
    ///
    /// A literal middle dot rather than `&middot;`: some chats show a title's entities as typed.
    pub(crate) fn page_title(&self) -> String {
        match (&self.title, &self.subtitle) {
            (Some(title), Some(subtitle)) => format!("{title} · {subtitle}"),
            (Some(title), None) => title.clone(),
            _ => format!("Wanda · Shared {}", self.noun),
        }
    }

    pub(crate) fn heading(&self) -> &str {
        self.title.as_deref().unwrap_or(match self.noun {
            "playlist" => "Open this playlist in Wanda",
            _ => "Connecting to track",
        })
    }

    pub(crate) fn subheading(&self) -> &str {
        self.subtitle
            .as_deref()
            .unwrap_or("Dispatching player intent to your device…")
    }

    /// The `<meta>` tags, escaped and ready to place in `<head>`.
    pub(crate) fn meta_tags(&self) -> String {
        let title = self
            .title
            .clone()
            .unwrap_or_else(|| format!("A {} shared from Wanda", self.noun));
        let description = match &self.subtitle {
            Some(subtitle) => format!("{subtitle} · Listen with Wanda"),
            None => "Open it in Wanda, or in your browser.".to_string(),
        };
        let og_type = if self.noun == "track" {
            "music.song"
        } else {
            "music.playlist"
        };
        let card = if self.image.is_some() {
            "summary_large_image"
        } else {
            "summary"
        };
        let mut tags = vec![
            meta("property", "og:site_name", "Wanda"),
            meta("property", "og:type", og_type),
            meta("property", "og:title", &title),
            meta("property", "og:description", &description),
            meta("name", "description", &description),
            meta("name", "twitter:card", card),
            meta("name", "twitter:title", &title),
            meta("name", "twitter:description", &description),
        ];
        if let Some(image) = &self.image {
            tags.push(meta("property", "og:image", image));
            tags.push(meta("name", "twitter:image", image));
        }
        tags.join("\n")
    }
}

fn meta(attribute: &str, key: &str, content: &str) -> String {
    format!(
        "<meta {attribute}=\"{key}\" content=\"{}\">",
        escape_html(content)
    )
}

/// The video id of a YouTube watch or `youtu.be` URL, checked for shape.
pub(super) fn youtube_video_id(target: &str) -> Option<String> {
    let url = url_host(target)?;
    let host = url.host.trim_start_matches("www.").trim_start_matches("m.");
    let after_host = target.strip_prefix("https://")?.split_once('/')?.1;
    let candidate = match host {
        "youtu.be" => after_host.split(['?', '#', '/']).next()?,
        "youtube.com" | "music.youtube.com" => after_host
            .split_once('?')?
            .1
            .split('#')
            .next()?
            .split('&')
            .find_map(|pair| pair.strip_prefix("v="))?,
        _ => return None,
    };
    is_video_id(candidate).then(|| candidate.to_string())
}
