//! GraphQL for artist subscriptions.
//!
//! Subscribing is by *name*, not by id. That is what a client actually holds: it is looking at a
//! track whose tag says a name, or a YouTube Music page whose channel it knows. Requiring an id
//! first would mean a lookup that can only fail for an artist nobody has published yet — which is
//! exactly the artist most worth being told about — so the name creates the row if it has to.
//!
//! There is no push here and there is not meant to be. The server has no way to reach a phone that
//! is not connected: `/ws/sync` is open only while the app is on screen, and nothing else in this
//! deployment speaks to a device. So the client polls [`ArtistQuery::new_releases`] from a
//! scheduled job and raises its own notification, carrying the watermark itself. That keeps the
//! server free of per-device delivery state, and works the same whether or not the phone has Google
//! Play Services on it.

use async_graphql::{Context, Object, Result, SimpleObject};

use crate::db::Db;
use crate::db_artists::Artist;
use crate::schema::{authorize, bounded};

/// An artist somebody can subscribe to.
#[derive(SimpleObject)]
pub struct ArtistSubscription {
    pub artist_id: String,
    /// The spelling to show — the first one the catalogue saw.
    pub display_name: String,
    /// What the name normalises to. The identity two spellings share; exposed so a client can tell
    /// that the artist it is looking at is one it already follows without asking again.
    pub norm_name: String,
    /// A namespaced id at the source it came from, such as `ytm:UC…`, when one is known.
    pub external_id: Option<String>,
}

impl From<Artist> for ArtistSubscription {
    fn from(artist: Artist) -> Self {
        Self {
            artist_id: artist.artist_id,
            display_name: artist.display_name,
            norm_name: artist.norm_name,
            external_id: artist.external_id,
        }
    }
}

/// Something a subscribed artist has put out since the client last looked.
#[derive(SimpleObject)]
pub struct ArtistRelease {
    pub recording_id: String,
    pub artist_id: String,
    pub artist: String,
    pub title: Option<String>,
    pub album: Option<String>,
    /// The catalogue position this was published at. The highest one seen is the next watermark.
    pub updated_at: i64,
}

#[derive(Default)]
pub struct ArtistQuery;

#[Object]
impl ArtistQuery {
    /// Who this account follows.
    async fn subscribed_artists(
        &self,
        ctx: &Context<'_>,
        user_id: String,
    ) -> Result<Vec<ArtistSubscription>> {
        authorize(ctx, &user_id)?;
        let db = ctx.data::<Db>()?;
        Ok(db
            .subscribed_artists(&user_id)?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    /// Whether this account follows the artist of a given name.
    ///
    /// For a page that has a name and no id — which is every artist page opened from a track.
    async fn is_subscribed_to_artist(
        &self,
        ctx: &Context<'_>,
        user_id: String,
        artist: String,
    ) -> Result<bool> {
        authorize(ctx, &user_id)?;
        let artist = bounded(&artist, 200, "artist")?;
        Ok(ctx.data::<Db>()?.is_subscribed_to(&user_id, &artist)?)
    }

    /// Releases by followed artists published after [`since`], oldest first.
    ///
    /// Oldest first because the client walks forward from a stored watermark. Newest-first would
    /// mean a client that had been away announced only the most recent release and silently
    /// skipped everything before it.
    async fn new_releases(
        &self,
        ctx: &Context<'_>,
        user_id: String,
        since: i64,
        #[graphql(default = 100)] limit: i64,
    ) -> Result<Vec<ArtistRelease>> {
        authorize(ctx, &user_id)?;
        let db = ctx.data::<Db>()?;
        Ok(db
            .releases_for_subscriber(&user_id, since, limit.clamp(1, 200))?
            .into_iter()
            .map(|release| ArtistRelease {
                recording_id: release.recording_id,
                artist_id: release.artist_id,
                artist: release.artist,
                title: release.title,
                album: release.album,
                updated_at: release.updated_at,
            })
            .collect())
    }
}

#[derive(Default)]
pub struct ArtistMutation;

#[Object]
impl ArtistMutation {
    /// Follows an artist, creating the row for them if the catalogue has never seen the name.
    ///
    /// Returns null for a name that normalises to nothing — punctuation, whitespace — rather than
    /// erroring: there is no artist to have followed, and nothing went wrong.
    async fn subscribe_artist(
        &self,
        ctx: &Context<'_>,
        user_id: String,
        artist: String,
        external_id: Option<String>,
    ) -> Result<Option<ArtistSubscription>> {
        authorize(ctx, &user_id)?;
        let artist = bounded(&artist, 200, "artist")?;
        let external_id = external_id
            .map(|id| bounded(&id, 200, "externalId"))
            .transpose()?;
        Ok(ctx
            .data::<Db>()?
            .subscribe_artist(&user_id, &artist, external_id.as_deref())?
            .map(Into::into))
    }

    /// Unfollows. False when there was nothing to unfollow, which is not an error either.
    async fn unsubscribe_artist(
        &self,
        ctx: &Context<'_>,
        user_id: String,
        artist_id: String,
    ) -> Result<bool> {
        authorize(ctx, &user_id)?;
        let artist_id = bounded(&artist_id, 200, "artistId")?;
        Ok(ctx.data::<Db>()?.unsubscribe_artist(&user_id, &artist_id)?)
    }
}
