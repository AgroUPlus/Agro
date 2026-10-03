//! Migrations 50 onwards. **Append only** — see the module comment in `migrations.rs`.

pub(super) const ENTRIES: &[&str] = &[
    // 50 — a shared playlist stays in sync, and its owner can let others edit it.
    //
    // `revision` counts every change to a playlist, so a client can say which version an edit was
    // made against and the server can refuse one made against a version that has since moved on.
    // `edit_access` is who besides the owner may change it: 0 nobody, 1 friends, 2 anyone who can
    // open it. Defaults to 0, so every playlist that existed stays owner-only — see
    // `playlist_access`. Each item now records who added it and when; the ones that predate this
    // were all added by the owner, so that is what they are given.
    //
    // `playlist_followers` is who keeps a live copy, which is who to tell when it changes.
    "ALTER TABLE playlists ADD COLUMN revision INTEGER NOT NULL DEFAULT 0;
     ALTER TABLE playlists ADD COLUMN edit_access INTEGER NOT NULL DEFAULT 0;
     ALTER TABLE playlist_items ADD COLUMN added_by TEXT;
     ALTER TABLE playlist_items ADD COLUMN added_at TEXT;
     UPDATE playlist_items SET
         added_by = (SELECT user_id FROM playlists WHERE playlists.id = playlist_items.playlist_id),
         added_at = (SELECT created_at FROM playlists WHERE playlists.id = playlist_items.playlist_id);

     CREATE TABLE IF NOT EXISTS playlist_followers (
         playlist_id  TEXT NOT NULL,
         user_id      TEXT NOT NULL,
         followed_at  TEXT NOT NULL,
         PRIMARY KEY (playlist_id, user_id),
         FOREIGN KEY (playlist_id) REFERENCES playlists(id) ON DELETE CASCADE
     );
     CREATE INDEX IF NOT EXISTS idx_playlist_followers_user ON playlist_followers(user_id);",
];
