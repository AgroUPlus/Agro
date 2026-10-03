//! Server features an operator can switch off.
//!
//! An Agro host decides how much of its machine to give away. Moving audio bytes, holding uploads,
//! fanning out presence to every friend on every heartbeat and running a jam clock all cost CPU,
//! disk or bandwidth that a small box may not have. Each of those is listed here, shown on the
//! dashboard's Management → Plugins page, and refused — explicitly, never faked — while it is off.
//!
//! State lives in `plugins_state`, keyed by [`Feature::id`]. A feature with no saved row is on, so
//! a fresh install and an upgraded one behave exactly as before until an admin says otherwise.

use async_graphql::ErrorExtensions;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use crate::db::Db;
use crate::plugins::{AgroPlugin, PluginMetaItem};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feature {
    AudioRelay,
    LibraryTransfers,
    PrivacyProxy,
    ShareLinks,
    FriendPresence,
    ListenAlong,
    Jams,
    AcousticSearch,
    Wrapped,
    PopularCharts,
}

/// The machine-readable code a refused call carries, so a client can hide the feature rather than
/// show a broken screen.
pub const DISABLED_CODE: &str = "FEATURE_DISABLED";

struct Descriptor {
    id: &'static str,
    name: &'static str,
    description: &'static str,
    category: &'static str,
    endpoint: &'static str,
    /// What switching it off saves, so an operator can choose with numbers in mind.
    cost: &'static str,
}

impl Feature {
    pub const ALL: [Feature; 10] = [
        Feature::AudioRelay,
        Feature::LibraryTransfers,
        Feature::PrivacyProxy,
        Feature::ShareLinks,
        Feature::FriendPresence,
        Feature::ListenAlong,
        Feature::Jams,
        Feature::AcousticSearch,
        Feature::Wrapped,
        Feature::PopularCharts,
    ];

    fn describe(self) -> Descriptor {
        match self {
            Feature::AudioRelay => Descriptor {
                id: "audio-relay",
                name: "Audio relay",
                description: "Streams audio between devices through this server when they cannot reach each other directly — listen-along and jams across networks.",
                category: "Bandwidth",
                endpoint: "/api/v1/relay",
                cost: "Every relayed stream passes through the server's uplink in full",
            },
            Feature::LibraryTransfers => Descriptor {
                id: "library-uploads",
                name: "Library uploads & transfers",
                description: "Accepts uploaded files for the library or the spool, and serves them back to the account's other devices.",
                category: "Bandwidth",
                endpoint: "/api/v1/library",
                cost: "Disk for the spool and library, and the upload and download traffic",
            },
            Feature::PrivacyProxy => Descriptor {
                id: "privacy-relay",
                name: "Privacy proxy",
                description: "Fetches metadata and lyrics (Internet Archive, LRCLIB, Nyaa) on the clients' behalf to hide their IP addresses, caching responses for a day.",
                category: "Bandwidth",
                endpoint: "/api/v1/proxy",
                cost: "Outbound requests on behalf of every client",
            },
            Feature::ShareLinks => Descriptor {
                id: "ephemeral-share",
                name: "Ephemeral share links",
                description: "Self-expiring share URLs served at /share/{token}.",
                category: "Sharing",
                endpoint: "/share/{token}",
                cost: "Small: one row per link",
            },
            Feature::FriendPresence => Descriptor {
                id: "friend-presence",
                name: "Friends' now playing",
                description: "Pushes each playing session to the friends who may see it, on every heartbeat. Accounts still choose for themselves whether to share.",
                category: "Social",
                endpoint: "/ws/sync",
                cost: "One push per friend per heartbeat — the largest share of CPU on a busy server",
            },
            Feature::ListenAlong => Descriptor {
                id: "listen-along",
                name: "Listen along",
                description: "Follow a friend's playback in real time, with a direct or relayed path to their audio.",
                category: "Social",
                endpoint: "/ws/sync",
                cost: "One push per listener per heartbeat",
            },
            Feature::Jams => Descriptor {
                id: "jams",
                name: "Jams",
                description: "Shared rooms where friends queue and vote on tracks, kept in step by a server-side clock.",
                category: "Social",
                endpoint: "/graphql",
                cost: "A clock tick every two seconds for each live room",
            },
            Feature::AcousticSearch => Descriptor {
                id: "acoustic-search",
                name: "Acoustic search",
                description: "Finds recordings that sound alike from the vectors clients submit.",
                category: "Discovery",
                endpoint: "/graphql",
                cost: "Vector storage, and a similarity scan per search",
            },
            Feature::Wrapped => Descriptor {
                id: "agro-wrapped",
                name: "Agro Wrapped",
                description: "The year-in-review built from an account's whole listening history.",
                category: "Discovery",
                endpoint: "/graphql",
                cost: "A full pass over the account's history per request",
            },
            Feature::PopularCharts => Descriptor {
                id: "popular-charts",
                name: "Popular on Agro",
                description: "A server-wide chart of what is being played, counted with no account attached and nothing shown below the exposure floor.",
                category: "Discovery",
                endpoint: "/api/v1/popular",
                cost: "Small: one counter per track per day",
            },
        }
    }

    pub fn id(self) -> &'static str {
        self.describe().id
    }

    pub fn from_id(id: &str) -> Option<Feature> {
        Feature::ALL.into_iter().find(|f| f.id() == id)
    }

    /// Whether the operator has left this on. A database error reads as on: switching a feature
    /// off is a deliberate act, and a failed read is not one.
    pub fn is_on(self, db: &Db) -> bool {
        db.plugin_state(self.id()).ok().flatten().unwrap_or(true)
    }

    /// The GraphQL refusal for a switched-off feature.
    pub fn require(self, db: &Db) -> async_graphql::Result<()> {
        if self.is_on(db) {
            return Ok(());
        }
        Err(async_graphql::Error::new(format!(
            "{} is turned off on this server.",
            self.describe().name
        ))
        .extend_with(|_, ext| {
            ext.set("code", DISABLED_CODE);
            ext.set("feature", self.id());
        }))
    }

    /// The HTTP refusal for a switched-off feature, or `None` to carry on.
    pub fn refuse_http(self, db: &Db) -> Option<Response> {
        if self.is_on(db) {
            return None;
        }
        Some(
            (
                StatusCode::FORBIDDEN,
                axum::Json(serde_json::json!({
                    "error": format!("{} is turned off on this server.", self.describe().name),
                    "code": DISABLED_CODE,
                    "feature": self.id(),
                })),
            )
                .into_response(),
        )
    }

    /// How the dashboard lists it.
    pub fn plugin(self, db: &Db) -> AgroPlugin {
        let d = self.describe();
        let on = self.is_on(db);
        AgroPlugin {
            id: d.id.to_string(),
            name: d.name.to_string(),
            description: d.description.to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            category: d.category.to_string(),
            target: "Server".to_string(),
            is_enabled: on,
            is_connected: on,
            toggleable: true,
            latency_ms: None,
            endpoint: Some(d.endpoint.to_string()),
            metadata: vec![PluginMetaItem {
                key: "Cost".to_string(),
                value: d.cost.to_string(),
            }],
        }
    }
}

/// Route middleware: refuses every request to a route while its feature is off.
///
/// A layer rather than a check inside each handler, because the handler's extractors run first: a
/// switched-off upload route would otherwise answer a malformed body with 422 before it ever got
/// round to saying the feature is off. Used as
/// `from_fn_with_state((db, Feature::AudioRelay), features::gate)`.
pub async fn gate(
    axum::extract::State((db, feature)): axum::extract::State<(Db, Feature)>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    match feature.refuse_http(&db) {
        Some(refused) => refused,
        None => next.run(request).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_feature_is_on_until_switched_off() {
        let db = Db::new_in_memory().unwrap();
        assert!(Feature::ALL.iter().all(|f| f.is_on(&db)));
        db.set_plugin_enabled("jams", false).unwrap();
        assert!(!Feature::Jams.is_on(&db));
        assert!(Feature::AudioRelay.is_on(&db));
    }

    #[test]
    fn a_refusal_names_the_feature_and_carries_the_code() {
        let db = Db::new_in_memory().unwrap();
        db.set_plugin_enabled("audio-relay", false).unwrap();
        let err = Feature::AudioRelay.require(&db).unwrap_err();
        let ext = err.extensions.expect("extensions");
        assert_eq!(
            ext.get("code"),
            Some(&async_graphql::Value::from(DISABLED_CODE))
        );
        assert!(Feature::AudioRelay.refuse_http(&db).is_some());
        assert!(Feature::Jams.refuse_http(&db).is_none());
    }

    #[test]
    fn ids_are_unique_and_round_trip() {
        for f in Feature::ALL {
            assert_eq!(Feature::from_id(f.id()), Some(f));
        }
        let mut ids: Vec<_> = Feature::ALL.iter().map(|f| f.id()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), Feature::ALL.len());
    }
}
