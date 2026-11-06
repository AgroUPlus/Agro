//! Share-link forwarding: the public half of the custom share domain.
//!
//! **This route implements `SHARE_LINKS.md`, which is normative.** Wanda mints these links and
//! frwd.top forwards them too; the parameter names and bounds below are duplicated in Kotlin and
//! JavaScript, and nothing in either toolchain will notice them drifting apart.
//!
//! A player rewrites its share links onto the domain the user set — `frwd.top/listen?v=<id>` for a
//! YouTube Music track, `?u=<url>` for anything else — and this route sends whoever opens one on
//! to the track. Point the domain's DNS at this server and the whole feature is Agro's; leave the
//! domain unset and the players share their backends' own links, which is what happens with no
//! Agro at all.
//!
//! Two rules this route does not break:
//!
//! 1. It forwards only to hosts an account here has allowed. A forwarder that will send a visitor
//!    to any address handed to it is an open redirect — a phishing URL wearing the user's domain,
//!    with that domain's reputation paying for it.
//! 2. It records nothing. No log line, no counter, no cookie. A shortener is a redirect log by
//!    construction, and the players this serves are built on the premise that nobody keeps one.

use crate::AppState;
use axum::{
    extract::{Query, State},
    http::{header, StatusCode},
    response::{Html, IntoResponse, Response},
};
use serde::Deserialize;

/// Public, unauthenticated, and logs nothing — see the module docs. **Implements
/// `SHARE_LINKS.md`, which is normative and shared with external Kotlin/JS clients: the
/// parameter names and bounds here are frozen.**
#[utoipa::path(
    get,
    path = "/listen",
    tag = "sharing",
    params(ListenParams),
    responses(
        (status = 200, description = "HTML page that forwards the browser to the resolved track link client-side", content_type = "text/html"),
        (status = 404, description = "No usable target (unknown short link, or a host not on the allow-list)", content_type = "text/html"),
    ),
)]
pub async fn listen_handler(
    Query(params): Query<ListenParams>,
    State(state): State<AppState>,
) -> Response {
    // Exactly one target (§3.1): a playlist id beside a track target is refused, not picked from.
    if let Some(playlist) = params.pl.as_deref() {
        let alone = params.id.is_none() && params.v.is_none() && params.u.is_none();
        return if alone && crate::listen_playlist::is_playlist_id(playlist) {
            crate::listen_playlist::page(
                playlist,
                &crate::listen_playlist::preview(&state, playlist),
            )
        } else {
            refusal()
        };
    }

    let target = match resolve(&params, &state) {
        Some(target) => target,
        // A link swept for going unused says so (§6a); anything else gets the refusal that says
        // nothing (§6). A failed lookup is the refusal too — it must not read as "deleted".
        None => {
            let retired = params.id.as_deref().is_some_and(|id| {
                state.db.short_link_retired(id).unwrap_or_else(|e| {
                    tracing::warn!("listen: tombstone lookup failed: {e}");
                    false
                })
            });
            return if retired {
                crate::listen_retired::page()
            } else {
                refusal()
            };
        }
    };

    let mut search_params = if let Some(id) = &params.id {
        format!("?id={}", id)
    } else if let Some(v) = &params.v {
        format!("?v={}", v)
    } else if let Some(u) = &params.u {
        format!("?u={}", urlencoding::encode(u))
    } else {
        String::new()
    };

    // Rebuilt rather than passed through, so only known parameters within known bounds reach the
    // app's URL. Forwarding the raw query string would put whatever a stranger appended into a
    // `wanda://` link, which is the same open-redirect mistake this route avoids for `u`.
    if !search_params.is_empty() {
        search_params.push_str(&playback_suffix(params.s, params.p));
    }

    let target_json = serde_json::to_string(&target).unwrap_or_else(|_| "\"\"".to_string());
    let search_json = serde_json::to_string(&search_params).unwrap_or_else(|_| "\"\"".to_string());

    let template = page::TEMPLATE;

    let preview = preview::track(&params, &state, &target);
    let html = template
        .replace("__TITLE__", &escape_html(&preview.page_title()))
        .replace("__PREVIEW_META__", &preview.meta_tags())
        .replace("__HEADING__", &escape_html(preview.heading()))
        .replace("__SUBHEADING__", &escape_html(preview.subheading()))
        .replace("__SEARCH_PARAMS__", &escape_html(&search_params))
        .replace("__TARGET__", &escape_html(&target))
        .replace("__TARGET_JSON__", &escape_script_json(&target_json))
        .replace("__SEARCH_JSON__", &escape_script_json(&search_json));

    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8".to_string()),
            (header::REFERRER_POLICY, "no-referrer".to_string()),
            (
                header::CACHE_CONTROL,
                "no-store, no-cache, must-revalidate, max-age=0".to_string(),
            ),
            (header::PRAGMA, "no-cache".to_string()),
        ],
        Html(html),
    )
        .into_response()
}

#[cfg(test)]
mod escaping_tests;

#[cfg(test)]
mod playback_tests;

#[cfg(test)]
mod preview_tests;

mod escaping;
mod page;
mod params;
pub(crate) mod preview;
mod refusal;
mod target;
pub(crate) use escaping::escape_html;
use escaping::escape_script_json;
use params::*;
use refusal::refusal;
use target::resolve;
#[cfg(test)]
mod tests;
