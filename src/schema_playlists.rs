//! GraphQL resolvers for source-agnostic playlists and external link ingestion.
//!
//! One file per concern under `schema_playlists/`: the payloads, reads, the original owner-only
//! writes, revision-checked edits, following, and the push that tells followers something changed.

mod edits;
mod follow;
mod mutation;
mod notify;
mod payload;
mod query;

#[derive(async_graphql::MergedObject, Default)]
pub struct PlaylistsQuery(query::PlaylistReadQuery, follow::FollowQuery);

#[derive(async_graphql::MergedObject, Default)]
pub struct PlaylistsMutation(
    mutation::PlaylistWriteMutation,
    edits::PlaylistEditsMutation,
    follow::FollowMutation,
);
