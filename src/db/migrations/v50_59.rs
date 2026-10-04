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
    // 51 — one short link per target, and links that lapse when nobody uses them.
    //
    // Every share minted a new row, so sending the same track ten times left ten links behind, and
    // none of them ever went unless it had named a deadline. `last_shared_at` records the owner
    // sending a link again, which counts as use alongside a click; the index is what finding the
    // existing link for a target is looked up by. `retired_short_links` keeps only the ids of
    // links swept for going unused — no target, no owner — so `/listen` can say the link was
    // deleted rather than that it never existed. See `db_short_links`.
    //
    // Shipped on `main` as 50 while collaborative playlists took 50 on the branch production
    // runs; production was already stamped 50 with the playlist columns, so this is the entry that
    // moved.
    "ALTER TABLE short_links ADD COLUMN last_shared_at INTEGER;
     CREATE INDEX IF NOT EXISTS idx_short_links_owner_target ON short_links(user_id, target_url);
     CREATE TABLE IF NOT EXISTS retired_short_links (
         id         TEXT PRIMARY KEY,
         retired_at INTEGER NOT NULL
     );",
    // 52 — what a jam was, kept for the people who were in it.
    //
    // A jam's own rows still go when it ends (`delete_jam`): a room is not a document. What
    // survives is one summary per member, written as they leave and holding only what that member
    // already saw in the room. It is theirs to dismiss, and the retention sweep removes whatever
    // they never do — see `db_jam_recap`.
    "CREATE TABLE IF NOT EXISTS jam_recaps (
         id           TEXT PRIMARY KEY,
         username     TEXT NOT NULL,
         payload_json TEXT NOT NULL,
         created_at   TEXT NOT NULL
     );
     CREATE INDEX IF NOT EXISTS idx_jam_recaps_user ON jam_recaps(username);",
    // 53 — Blends: playlists Agro writes from several friends' listening.
    //
    // `kind` marks a playlist nobody edits by hand; every existing one is 'manual'. A blend is a
    // playlist row like any other — so sharing, following and revision polling work unchanged —
    // plus its recipe in `blends` and who is in it in `blend_members`. A member is 'invited' until
    // they accept: their listening is never read before then. See `blend` and `db_blend`.
    "ALTER TABLE playlists ADD COLUMN kind TEXT NOT NULL DEFAULT 'manual';
     CREATE TABLE IF NOT EXISTS blends (
         playlist_id  TEXT PRIMARY KEY,
         size         INTEGER NOT NULL,
         mix          INTEGER NOT NULL,
         time_window  TEXT NOT NULL,
         refresh      TEXT NOT NULL,
         refreshed_at TEXT
     );
     CREATE TABLE IF NOT EXISTS blend_members (
         playlist_id TEXT NOT NULL,
         username    TEXT NOT NULL,
         state       TEXT NOT NULL,
         invited_at  TEXT NOT NULL,
         PRIMARY KEY (playlist_id, username)
     );
     CREATE INDEX IF NOT EXISTS idx_blend_members_user ON blend_members(username);",
    // 54 — the cloud vault: a device's backup, sealed before it left the device.
    //
    // `blob` is AES-256-GCM under a key derived from the account's vault key, which this server
    // never holds; it is stored and handed back, never read. Everything else is the label on the
    // envelope, written by the device so the dashboard can say what a backup is without opening
    // it: when, from which device, how big, and which sections with how many records each — never
    // a setting, a title or a name. `sha256` is of the sealed bytes, so a download can be checked.
    // Only the newest few per account are kept — see `db_vault`.
    "CREATE TABLE IF NOT EXISTS vault_backups (
         id                TEXT PRIMARY KEY,
         user_id           TEXT NOT NULL,
         created_at        TEXT NOT NULL,
         device_id         TEXT NOT NULL,
         device_name       TEXT,
         app_version       TEXT,
         format            INTEGER NOT NULL,
         plain_bytes       INTEGER NOT NULL,
         sealed_bytes      INTEGER NOT NULL,
         sections_json     TEXT NOT NULL,
         includes_accounts INTEGER NOT NULL,
         sha256            TEXT NOT NULL,
         blob              BLOB NOT NULL
     );
     CREATE INDEX IF NOT EXISTS idx_vault_backups_user ON vault_backups(user_id, created_at);",
];
