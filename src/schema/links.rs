//! Short links and ephemeral shares, from the dashboard's side.

use crate::db::Db;
use crate::db::LinkKind;
use async_graphql::{Context, Object, SimpleObject};

use super::{authorize, caller, public_url, MAX_URL_LEN};

#[derive(SimpleObject, Clone)]
pub struct SharePayload {
    pub token: String,
    pub share_url: String,
    pub expires_at: String,
    pub track_title: String,
    pub artist_name: String,
}

/// One link this account has minted, whichever of the two mechanisms produced it.
#[derive(SimpleObject, Clone)]
pub struct ShareLink {
    pub id: String,
    /// `SHORT` for `/listen?id=…`, `EPHEMERAL` for a hosted `/share/<token>` page.
    pub kind: String,
    /// Where the link goes: the forwarding target, or the hosted audio URL.
    pub target: String,
    /// The full address to hand out.
    pub url: String,
    /// What the link is of, when the row knows. Only ephemeral shares carry track metadata.
    pub label: Option<String>,
    pub created_at: Option<i64>,
    pub expires_at: Option<i64>,
    /// How many times it has been opened. An aggregate and nothing else — see migration 6.
    pub click_count: i64,
    pub last_clicked_at: Option<i64>,
    /// Which backend minted the underlying share, when known. `"navidrome"` matters at deletion.
    pub source: Option<String>,
}

/// The outcome of deleting a link.
#[derive(SimpleObject, Clone)]
pub struct DeleteLinkPayload {
    pub deleted: bool,
    /// True when the link pointed at a Navidrome share that Agro cannot revoke on the user's
    /// behalf.
    ///
    /// Agro holds a Navidrome address and username but deliberately never the password — see the
    /// encrypted fields on `synced_settings`, and the "the password stays on each device" rule the
    /// clients are built around. Revoking a share needs that password, so the honest answer is to
    /// remove Agro's own record and say plainly that the share still exists on the music server,
    /// rather than to start storing a credential the whole design avoids.
    pub navidrome_cleanup_required: bool,
}

#[derive(Default)]
pub struct LinksQuery;

#[Object]
impl LinksQuery {
    /// Looks up the target URL for a short link UID.
    ///
    /// Authenticated: the public half of this lives at `/listen`, which is the capability URL
    /// people without an account open. This resolver is the dashboard's, and took no token, so any
    /// caller could walk other accounts' links.
    async fn resolve_short_link(
        &self,
        ctx: &Context<'_>,
        id: String,
    ) -> async_graphql::Result<Option<String>> {
        caller(ctx)?;
        let db = ctx.data::<Db>()?;
        let target = db.get_short_link(&id)?;
        Ok(target)
    }

    /// Every link this account has minted, newest first.
    ///
    /// Both mechanisms in one list: the user made "a link", and which table it landed in is an
    /// implementation detail they should not have to know to find it again.
    async fn links(
        &self,
        ctx: &Context<'_>,
        user_id: String,
    ) -> async_graphql::Result<Vec<ShareLink>> {
        authorize(ctx, &user_id)?;
        let db = ctx.data::<Db>()?;
        let base = public_url();
        let base = base.trim_end_matches('/');
        Ok(db
            .list_links(&user_id)?
            .into_iter()
            .map(|row| ShareLink {
                url: match row.kind {
                    LinkKind::Short => format!("{base}/listen?id={}", row.id),
                    LinkKind::Ephemeral => format!("{base}/share/{}", row.id),
                },
                kind: match row.kind {
                    LinkKind::Short => "SHORT".to_string(),
                    LinkKind::Ephemeral => "EPHEMERAL".to_string(),
                },
                id: row.id,
                target: row.target,
                label: row.label,
                created_at: row.created_at,
                expires_at: row.expires_at,
                click_count: row.click_count,
                last_clicked_at: row.last_clicked_at,
                source: row.source,
            })
            .collect())
    }
}

#[derive(Default)]
pub struct LinksMutation;

#[Object]
impl LinksMutation {
    /// Creates a short UID for a share URL. Returns the short link UID (e.g. "aB3x9Q"), or the
    /// account's existing open-ended link to the same target — see `db_short_links`.
    ///
    /// `source` records which backend minted the underlying share — `"navidrome"` when the link
    /// points at a Navidrome share, so deleting it later can also revoke it there.
    async fn create_short_link(
        &self,
        ctx: &Context<'_>,
        user_id: String,
        target_url: String,
        source: Option<String>,
        expires_at: Option<i64>,
    ) -> async_graphql::Result<String> {
        // A link attributed to an account has to be authorised as that account. `userId` used to
        // be optional, and omitting it skipped this check entirely — minting an unowned link that
        // no account could then list or revoke.
        authorize(ctx, &user_id)?;
        let db = ctx.data::<Db>()?;
        let target_url = target_url.trim();
        if target_url.is_empty() {
            return Err("Target URL cannot be empty".into());
        }
        if target_url.chars().count() > MAX_URL_LEN {
            return Err(format!("A URL may be at most {MAX_URL_LEN} characters").into());
        }
        // A forwarder that will point at any scheme is a phishing primitive wearing the operator's
        // domain. `/listen` checks the host against an allowlist; this checks the scheme.
        if !target_url.starts_with("https://") && !target_url.starts_with("http://") {
            return Err("A link target must be an http or https URL".into());
        }
        // The same target shared again gets the link it already has, rather than one more row.
        if expires_at.is_none() {
            if let Some(existing) = db.reuse_short_link(&user_id, target_url, source.as_deref())? {
                return Ok(existing);
            }
        }
        use rand::Rng;
        const CHARSET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
        let mut rng = rand::thread_rng();
        let uid: String = (0..7)
            .map(|_| {
                let idx = rng.gen_range(0..CHARSET.len());
                CHARSET[idx] as char
            })
            .collect();

        db.create_short_link(
            &uid,
            target_url,
            Some(user_id.as_str()),
            source.as_deref(),
            expires_at,
        )?;
        Ok(uid)
    }

    /// Removes a link so it stops resolving.
    ///
    /// `kind` is the discriminator from `links` — `SHORT` or `EPHEMERAL`. Deleting is scoped to the
    /// owning account inside the statement itself, so a link belonging to somebody else is a
    /// not-found rather than a deletion.
    async fn delete_link(
        &self,
        ctx: &Context<'_>,
        user_id: String,
        id: String,
        kind: String,
    ) -> async_graphql::Result<DeleteLinkPayload> {
        authorize(ctx, &user_id)?;
        let db = ctx.data::<Db>()?;
        let kind = match kind.to_ascii_uppercase().as_str() {
            "SHORT" => LinkKind::Short,
            "EPHEMERAL" => LinkKind::Ephemeral,
            other => return Err(format!("Unknown link kind: {other}").into()),
        };

        let source = db
            .delete_link(&user_id, &id, kind)
            .map_err(|_| async_graphql::Error::new("No such link on this account"))?;

        Ok(DeleteLinkPayload {
            deleted: true,
            navidrome_cleanup_required: source.as_deref() == Some("navidrome"),
        })
    }

    // `createAccount` used to live here: an administrator could mint an account directly, with a
    // passphrase handed back in the response and the account active immediately. It is gone
    // deliberately. Accounts come from `POST /api/v1/signup` and nowhere else, so that every
    // account is subject to the same rules — the username check, the rate limiter, the approval
    // queue — rather than those rules applying to strangers and not to the people an admin adds.
    // An admin who wants to let someone in mints an invite code, which skips the queue without
    // skipping the process.

    #[allow(clippy::too_many_arguments)]
    async fn create_ephemeral_share(
        &self,
        ctx: &Context<'_>,
        user_id: String,
        track_title: String,
        artist_name: String,
        album_name: Option<String>,
        audio_url: String,
        ttl_hours: Option<i64>,
    ) -> async_graphql::Result<SharePayload> {
        authorize(ctx, &user_id)?;
        let db = ctx.data::<Db>()?;
        crate::features::Feature::ShareLinks.require(db)?;
        let ttl = ttl_hours.unwrap_or(24);
        let token = db.create_ephemeral_share(
            &user_id,
            &track_title,
            &artist_name,
            album_name.as_deref(),
            &audio_url,
            ttl,
        )?;
        // Was hardcoded to localhost, which made every ephemeral share unopenable from any device
        // but the server itself.
        let share_url = format!("{}/share/{}", public_url().trim_end_matches('/'), token);
        let expires_at = (chrono::Utc::now() + chrono::Duration::hours(ttl)).to_rfc3339();

        Ok(SharePayload {
            token,
            share_url,
            expires_at,
            track_title,
            artist_name,
        })
    }
}
