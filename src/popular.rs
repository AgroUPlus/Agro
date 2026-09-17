//! Public, unauthenticated mirror of `popularTracks` — see [`crate::schema_popularity`].
//!
//! The GraphQL query sits behind a bearer token only because `/graphql` itself does (opening it
//! to the world would expose the whole schema, parser and executor — see `crate::login`'s module
//! doc). The data underneath is already structurally anonymous, with an exposure floor applied
//! before anything is named (see `crate::db_popularity`'s module doc), so a REST mirror with no
//! auth check discloses nothing that authentication was protecting. This is what a logged-out
//! caller — chiefly the docs site's static Charts page — fetches instead.

use axum::{
    extract::{ConnectInfo, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::time::Duration;

use crate::AppState;

/// How many requests one address gets per window.
///
/// Far more generous than login's budget: this is page-view traffic, not a credential guess, and
/// a rapid retry cannot learn anything a single call did not already say. Generous enough for a
/// visitor to click between all three of the docs page's day-window tabs without tripping it, but
/// still bounded against naive scraping.
const BUDGET: usize = 60;
const WINDOW: Duration = Duration::from_secs(300);

#[derive(Deserialize)]
pub struct PopularParams {
    days: Option<i64>,
    limit: Option<i64>,
}

/// One recording in the response body.
#[derive(Serialize, utoipa::ToSchema)]
pub struct PopularTrackJson {
    title: String,
    artist: String,
    album: Option<String>,
    /// Plays across the whole window, from everyone, attributed to nobody.
    count: i64,
    /// Looked up from title and artist alone — see `crate::cover_lookup`'s module doc for why
    /// this is safe where a catalogue join would not be. Absent if nothing was found.
    cover_url: Option<String>,
}

/// The whole response body.
#[derive(Serialize, utoipa::ToSchema)]
pub struct PopularResponse {
    /// Whether the "popular-charts" plugin is on for this server. `false` means every other field
    /// is a placeholder, not "nothing charted yet".
    enabled: bool,
    days: i64,
    tracks: Vec<PopularTrackJson>,
}

/// Public, unauthenticated. The same fleet-wide chart `popularTracks` serves, for callers with no
/// account — chiefly the docs site's static Charts page.
#[utoipa::path(
    get,
    path = "/api/v1/popular",
    tag = "popular",
    params(
        ("days" = Option<i64>, Query, description = "Window size in days, clamped to [1, 30]. Defaults to 7."),
        ("limit" = Option<i64>, Query, description = "Max recordings returned, clamped to [1, 100]. Defaults to 20."),
    ),
    responses(
        (status = 200, description = "The chart, or `{enabled: false}` if the plugin is turned off", body = PopularResponse),
        (status = 429, description = "Too many requests from this address", body = crate::openapi::ApiError),
    ),
)]
pub async fn popular_handler(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(params): Query<PopularParams>,
) -> Response {
    let ip = crate::login::client_ip(addr, &headers);
    if !state.popular_rate_limiter.charge(&ip, 1, BUDGET, WINDOW) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({"error": "too many requests"})),
        )
            .into_response();
    }

    // Disabled fleet-wide: an explicit `enabled: false` rather than an error, so the docs page can
    // render its "charts are turned off" state instead of special-casing this call.
    if !crate::plugins::is_enabled(&state.db, "popular-charts") {
        return Json(PopularResponse {
            enabled: false,
            days: 0,
            tracks: Vec::new(),
        })
        .into_response();
    }

    let days = params
        .days
        .unwrap_or(7)
        .clamp(1, crate::db_popularity::RETENTION_DAYS);
    let limit = params.limit.unwrap_or(20).clamp(1, 100) as usize;

    match state.db.popular_tracks(today(), days, limit) {
        Ok(tracks) => {
            // Bounded by `limit` (clamped above to at most 100) and, past the first request for a
            // given recording, answered from cache — see `cover_lookup`'s module doc.
            let covers = futures_util::future::join_all(tracks.iter().map(|track| {
                crate::cover_lookup::find_cover(
                    &state.http_client,
                    &state.db,
                    &track.artist,
                    &track.title,
                )
            }))
            .await;
            let tracks = tracks
                .into_iter()
                .zip(covers)
                .map(|(track, cover_url)| PopularTrackJson {
                    title: track.title,
                    artist: track.artist,
                    album: track.album,
                    count: track.count,
                    cover_url,
                })
                .collect();
            Json(PopularResponse {
                enabled: true,
                days,
                tracks,
            })
            .into_response()
        }
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("could not read the chart: {err}")})),
        )
            .into_response(),
    }
}

/// Whole days since the epoch, UTC. Same one-liner as `crate::schema_popularity::today`.
fn today() -> i64 {
    chrono::Utc::now().timestamp() / 86_400
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db_popularity::CountIncrement;
    use crate::login::tests::test_state;

    fn addr() -> ConnectInfo<SocketAddr> {
        ConnectInfo("203.0.113.9:5000".parse().unwrap())
    }

    async fn call(state: AppState, params: PopularParams) -> Response {
        popular_handler(State(state), addr(), HeaderMap::new(), Query(params)).await
    }

    #[tokio::test]
    async fn a_disabled_plugin_answers_with_an_explicit_flag_not_an_error() {
        let db = crate::db::Db::new_in_memory().unwrap();
        db.set_plugin_enabled("popular-charts", false).unwrap();
        let state = test_state(db);

        let response = call(
            state,
            PopularParams {
                days: None,
                limit: None,
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn an_address_past_the_budget_is_refused() {
        let db = crate::db::Db::new_in_memory().unwrap();
        let state = test_state(db);

        for _ in 0..BUDGET {
            let response = call(
                state.clone(),
                PopularParams {
                    days: None,
                    limit: None,
                },
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
        }
        let response = call(
            state,
            PopularParams {
                days: None,
                limit: None,
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn only_recordings_above_the_exposure_floor_are_named() {
        let db = crate::db::Db::new_in_memory().unwrap();
        db.add_play_counts(
            today(),
            &[CountIncrement {
                title: "All I Need".to_string(),
                artist: "Radiohead".to_string(),
                album: Some("In Rainbows".to_string()),
                count: crate::db_popularity::MIN_EXPOSURE_COUNT,
            }],
        )
        .unwrap();
        db.add_play_counts(
            today(),
            &[CountIncrement {
                title: "Weird Fishes".to_string(),
                artist: "Radiohead".to_string(),
                album: None,
                count: 1,
            }],
        )
        .unwrap();
        // Pre-seeds the cover cache so this test never makes a real network call to iTunes —
        // see `crate::cover_lookup`'s tests for the lookup logic itself.
        let cover_url = format!(
            "https://itunes.apple.com/search?term={}&media=music&entity=song&limit=1",
            urlencoding::encode("Radiohead All I Need")
        );
        db.set_cached_proxy(
            &cover_url,
            "{}",
            b"https://example.com/cover.jpg",
            9_999_999_999,
        )
        .unwrap();
        let state = test_state(db);

        let response = call(
            state,
            PopularParams {
                days: None,
                limit: None,
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["enabled"], true);
        let tracks = parsed["tracks"].as_array().unwrap();
        assert_eq!(tracks.len(), 1, "the once-played track must not be named");
        assert_eq!(tracks[0]["cover_url"], "https://example.com/cover.jpg");
        assert_eq!(tracks[0]["title"], "All I Need");
    }
}
