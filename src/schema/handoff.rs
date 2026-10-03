//! The playback handoff: reporting what a device is playing, and reading it back.

use crate::db::Db;
use crate::ws::WsHub;
use async_graphql::{Context, Object, SimpleObject};
use std::sync::Arc;

use super::handoff_input::{
    HandoffInput, HandoffTrackInput, MAX_PRESENCE_CIPHERTEXTS, MAX_QUEUE_TRACKS,
    MAX_SEALED_PAYLOAD_LEN,
};
use super::{authorize, bounded, normalise_username};

#[derive(SimpleObject, Clone)]
pub struct HandoffState {
    pub track_uri: String,
    /// How long the track is. 0 when the sender did not say, or when it is a livestream — both
    /// want a running clock rather than a progress bar that finishes at the wrong moment.
    pub duration_ms: i64,
    pub track_title: String,
    pub artist_name: String,
    pub album_name: Option<String>,
    pub artwork_url: Option<String>,
    pub position_ms: i64,
    pub is_playing: bool,
    pub device_id: String,
    pub updated_at: String,
    /// The rest of the session: every track in the queue, so picking it up on another device
    /// continues the listening rather than playing one song and stopping.
    pub queue: Vec<HandoffTrack>,
    /// Where `queue` was playing. -1 when the sender reported no queue at all.
    pub queue_index: i32,
    /// Present only for a sealed session: an authenticated envelope the server forwards without
    /// opening. A client with the account's vault key unseals the real metadata; everything else
    /// shows the session as private.
    pub encrypted_payload: Option<String>,
}

/// One entry of a handed-over queue. `track_uri` is the sending client's own id for it — a
/// receiving client resolves it against its own backends, falling back to title and artist when
/// the two devices do not share that source.
#[derive(SimpleObject, Clone, serde::Serialize, serde::Deserialize)]
pub struct HandoffTrack {
    pub track_uri: String,
    pub track_title: String,
    pub artist_name: String,
    pub album_name: Option<String>,
    pub artwork_url: Option<String>,
}

#[derive(Default)]
pub struct HandoffQuery;

#[Object]
impl HandoffQuery {
    /// Where the account left off.
    ///
    /// `excludeDevice` asks the question a client asks about *the rest of* its fleet: give me the
    /// latest session that is not mine. Optional, so a client that only wants "where was I" —
    /// which is most of them — is unchanged.
    async fn playback_handoff(
        &self,
        ctx: &Context<'_>,
        user_id: String,
        exclude_device: Option<String>,
    ) -> async_graphql::Result<Option<HandoffState>> {
        authorize(ctx, &user_id)?;
        let db = ctx.data::<Db>()?;
        let rec = match exclude_device
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
        {
            Some(device_id) => db.get_handoff_excluding(&user_id, device_id)?,
            None => db.get_handoff(&user_id)?,
        };
        Ok(rec.map(|r| HandoffState {
            track_uri: r.track_uri,
            duration_ms: r.duration_ms,
            track_title: r.track_title,
            artist_name: r.artist_name,
            album_name: r.album_name,
            artwork_url: r.artwork_url,
            position_ms: r.position_ms,
            is_playing: r.is_playing,
            device_id: r.device_id,
            updated_at: r.updated_at,
            // Stored opaquely as JSON; a value written by an older client that predates the queue
            // simply reads back as an empty one rather than failing the whole query.
            queue: r
                .queue_json
                .and_then(|json| serde_json::from_str::<Vec<HandoffTrack>>(&json).ok())
                .unwrap_or_default(),
            queue_index: r.queue_index.unwrap_or(-1) as i32,
            encrypted_payload: r.encrypted_payload,
        }))
    }
}

#[derive(Default)]
pub struct HandoffMutation;

#[Object]
impl HandoffMutation {
    async fn update_handoff(
        &self,
        ctx: &Context<'_>,
        input: HandoffInput,
    ) -> async_graphql::Result<bool> {
        authorize(ctx, &input.user_id)?;
        let db = ctx.data::<Db>()?;
        // A queue is capped rather than rejected: an endless-radio client can hold hundreds of
        // entries, and the first hundred is far more session than anyone resumes through.
        let queue_json = input.queue.as_ref().map(|tracks| {
            let capped: Vec<&HandoffTrackInput> = tracks.iter().take(MAX_QUEUE_TRACKS).collect();
            serde_json::to_string(&capped).unwrap_or_else(|_| "[]".to_string())
        });

        // Sealed bytes are opaque to the server, which is the point of them and also the reason
        // they need a size limit: nothing downstream can look at one and decide it is unreasonable.
        if let Some(payload) = input.encrypted_payload.as_deref() {
            bounded(payload, MAX_SEALED_PAYLOAD_LEN, "encryptedPayload")?;
        }
        let presence_ciphertexts = match input.presence_ciphertexts.as_ref() {
            None => None,
            Some(copies) => {
                if copies.len() > MAX_PRESENCE_CIPHERTEXTS {
                    return Err(format!(
                        "A handoff may carry at most {MAX_PRESENCE_CIPHERTEXTS} sealed copies"
                    )
                    .into());
                }
                let mut sealed = Vec::with_capacity(copies.len());
                for copy in copies {
                    // The recipient is validated as a username rather than taken as given: it is a
                    // key in a table keyed by it, and it is the field that decides who is later
                    // handed this ciphertext.
                    let recipient = normalise_username(&copy.recipient_user_id)?;
                    let device = bounded(&copy.recipient_device_id, 128, "recipientDeviceId")?;
                    if device.is_empty() {
                        return Err("A sealed copy must name the device it was sealed to".into());
                    }
                    sealed.push(crate::db_presence::PresenceCiphertext {
                        recipient_user_id: recipient,
                        recipient_device_id: device,
                        ciphertext: bounded(
                            &copy.ciphertext,
                            MAX_SEALED_PAYLOAD_LEN,
                            "ciphertext",
                        )?,
                    });
                }
                Some(sealed)
            }
        };

        db.update_handoff(
            &input.user_id,
            &input.track_uri,
            &input.track_title,
            &input.artist_name,
            input.album_name.as_deref(),
            input.artwork_url.as_deref(),
            input.position_ms,
            input.duration_ms.unwrap_or(0).max(0),
            input.is_playing,
            &input.device_id,
            queue_json.as_deref(),
            input.queue_index.map(|i| i as i64),
            input.content_hash.as_deref(),
            input.encrypted_payload.as_deref(),
            presence_ciphertexts.as_deref(),
        )?;

        let track_summary = format!("{} • {}", input.track_title, input.artist_name);
        // A handoff reports what is playing, not what the device is called: the name it already
        // has stands, and the invented one is only for a device seen here first.
        let petname = crate::passphrase::generate_random_petname();
        let client_type = if input.device_id.to_lowercase().contains("android")
            || input.device_id.to_lowercase().contains("wanda")
        {
            "wanda"
        } else {
            "wander"
        };
        let _ = db.upsert_node(
            &input.device_id,
            &input.user_id,
            crate::db::NodeName::KeepOr(&petname),
            client_type,
            None,
            Some(&track_summary),
        );

        if let Ok(ws_hub) = ctx.data::<Arc<WsHub>>() {
            ws_hub.notify_user(
                &input.user_id,
                "HANDOFF",
                serde_json::json!({
                    "trackTitle": input.track_title,
                    "artistName": input.artist_name,
                    "albumName": input.album_name,
                    "positionMs": input.position_ms,
                    "isPlaying": input.is_playing,
                    "deviceId": input.device_id,
                    "petname": petname,
                    "encryptedPayload": input.encrypted_payload,
                }),
            );
            crate::schema_social::fan_out_presence(db, ws_hub, &input.user_id);
        }

        Ok(true)
    }

    // ── Library ─────────────────────────────────────────────────────────────────────────────
}
