import { gql } from './api.js';

/**
 * Playlists and albums for the Library tab: the queries, and one place that turns a GraphQL error
 * into something a screen can act on. A stale edit comes back with `code === 'STALE_REVISION'` and
 * the revision the playlist is at now, so the caller can refetch and try again rather than fail.
 */

const ITEM_FIELDS = `id title artist album durationMs artworkUrl coverKey addedBy addedAt`;
const PLAYLIST_FIELDS = `id userId title description visibility editAccess myRole isFollowing
  revision itemCount totalDurationMs updatedAt items { ${ITEM_FIELDS} }`;

async function run(query, variables) {
  const res = await gql(query, variables);
  const body = await res.json();
  const error = body?.errors?.[0];
  if (error) {
    const failure = new Error(error.message);
    failure.code = error.extensions?.code;
    failure.currentRevision = error.extensions?.currentRevision;
    throw failure;
  }
  return body.data;
}

export async function listPlaylists() {
  const data = await run(`query { playlists { ${PLAYLIST_FIELDS} } }`);
  return data.playlists;
}

export async function listFollowed() {
  const data = await run(`query { followedPlaylists { playlists { ${PLAYLIST_FIELDS} } revokedIds } }`);
  return data.followedPlaylists;
}

export async function getPlaylist(id) {
  const data = await run(`query($id: String!) { playlist(id: $id) { ${PLAYLIST_FIELDS} } }`, { id });
  return data.playlist;
}

/** The current revision of one playlist, or null when it is gone or no longer shared with us. */
export async function playlistRevision(id) {
  const data = await run(
    `query($ids: [String!]!) { playlistRevisions(ids: $ids) { revision accessible } }`,
    { ids: [id] }
  );
  const entry = data.playlistRevisions[0];
  return entry?.accessible ? entry.revision : null;
}

/** `edits` are `{ add: { track, afterItemId } }`, `{ remove: itemId }` or `{ move: { itemId, afterItemId } }`. */
export async function applyEdits(id, baseRevision, edits) {
  const data = await run(
    `mutation($id: String!, $base: Int!, $edits: [PlaylistEditInput!]!) {
       applyPlaylistEdits(playlistId: $id, baseRevision: $base, edits: $edits) { ${PLAYLIST_FIELDS} }
     }`,
    { id, base: baseRevision, edits }
  );
  return data.applyPlaylistEdits;
}

export async function updateDetails(id, baseRevision, title, description) {
  const data = await run(
    `mutation($id: String!, $base: Int!, $title: String!, $description: String) {
       updatePlaylistDetails(playlistId: $id, baseRevision: $base, title: $title, description: $description) { ${PLAYLIST_FIELDS} }
     }`,
    { id, base: baseRevision, title, description: description || null }
  );
  return data.updatePlaylistDetails;
}

export async function setVisibility(id, visibility) {
  await run(
    `mutation($id: String!, $v: PlaylistVisibility) { updatePlaylistVisibility(playlistId: $id, visibility: $v) }`,
    { id, v: visibility }
  );
}

export async function setEditAccess(id, editAccess) {
  const data = await run(
    `mutation($id: String!, $a: EditAccess!) { updatePlaylistEditAccess(playlistId: $id, editAccess: $a) }`,
    { id, a: editAccess }
  );
  return data.updatePlaylistEditAccess;
}

export async function follow(id) {
  await run(`mutation($id: String!) { followPlaylist(id: $id) { id } }`, { id });
}

export async function unfollow(id) {
  await run(`mutation($id: String!) { unfollowPlaylist(id: $id) }`, { id });
}

export async function deletePlaylist(id) {
  await run(`mutation($id: String!) { deletePlaylist(playlistId: $id) }`, { id });
}

export async function createPlaylist(title) {
  const data = await run(
    `mutation($t: String!) { createPlaylist(title: $t, visibility: PRIVATE) { id } }`,
    { t: title }
  );
  return data.createPlaylist.id;
}

/** Tracks in the account's library matching `search`, to add to a playlist. */
export async function searchTracks(username, search) {
  const data = await run(
    `query($u: String!, $s: String) {
       libraryBrowse(userId: $u, kind: TRACK, search: $s, limit: 25) { id title subtitle coverKey }
     }`,
    { u: username, s: search }
  );
  return data.libraryBrowse;
}

export async function listAlbums(username, limit = 18) {
  const data = await run(
    `query($u: String!, $l: Int) {
       libraryBrowse(userId: $u, kind: ALBUM, limit: $l) { id title subtitle coverKey trackCount }
     }`,
    { u: username, l: limit }
  );
  return data.libraryBrowse;
}

export async function getAlbum(username, id) {
  const data = await run(
    `query($u: String!, $id: String!) {
       libraryAlbum(userId: $u, id: $id) {
         id title artist coverKey year totalDurationMs
         tracks { id title artist trackNo discNo durationMs }
       }
     }`,
    { u: username, id }
  );
  return data.libraryAlbum;
}

export const coverUrl = (key) => (key ? `/api/v1/cover/${key}` : null);
