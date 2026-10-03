//! Migrations 42 to 49. See [`super::MIGRATIONS`].

pub(super) const ENTRIES: &[&str] = &[
    // 42 — a handoff can be end-to-end encrypted.
    //
    // When a client holds a vault key, the real track metadata travels only inside this
    // authenticated envelope — the plaintext columns carry a placeholder — and another of the
    // account's devices unseals it. NULL for every ordinary handoff, which is nearly all of them.
    //
    // Appended rather than slotted in beside the other handoff migrations, and that placement is
    // the point. `migrate` stamps `PRAGMA user_version` from this array's *index* and skips
    // everything at or below the stamp it finds, so an entry's version **is** its position.
    // Inserting one anywhere but the end renumbers every entry after it, onto numbers that have
    // already been stamped in the field — and every database past that point then skips the new
    // entry forever and re-runs its neighbour instead.
    //
    // Placed beside the handoff migrations, this column was never added to any database that had
    // already run the device-key registry: the column would simply not exist, and every query
    // naming it would fail at runtime. No test could see it, because a fresh database applies the
    // whole list in order whatever order it is in. This list is append-only, and
    // `migrations_are_append_only` is what holds it to that.
    "ALTER TABLE handoff_state ADD COLUMN encrypted_payload TEXT;",
    // 43 — the same session, sealed once per friend device rather than once per account.
    //
    // `encrypted_payload` above is sealed symmetrically, under a subkey derived from the account's
    // own vault key, so only the account's other devices can open it. That made every sealed
    // session read as "Private Session" to friends: the social feed reads the plaintext columns,
    // and those now hold a placeholder. Encrypting the session and being seen by friends were
    // mutually exclusive.
    //
    // A row here is one copy of that same metadata, sealed to one friend device's public key, the
    // way `drop_note_ciphertexts` seals a note. The server stores and hands out copies it cannot
    // open, and each viewer is given only the one addressed to the device it is asking from.
    //
    // Keyed by sender device as well as sender: two of the account's devices can be publishing at
    // once, and a copy belongs to the session that produced it.
    "CREATE TABLE IF NOT EXISTS handoff_presence_ciphertexts (
         user_id             TEXT NOT NULL,
         device_id           TEXT NOT NULL,
         recipient_user_id   TEXT NOT NULL,
         recipient_device_id TEXT NOT NULL,
         ciphertext          TEXT NOT NULL,
         created_at          TEXT NOT NULL,
         PRIMARY KEY (user_id, device_id, recipient_user_id, recipient_device_id)
     );
     CREATE INDEX IF NOT EXISTS idx_handoff_presence_recipient
         ON handoff_presence_ciphertexts(recipient_user_id, recipient_device_id);",
    // 44 — the catalogue identifies a recording by its embedding, not by sub-hashes.
    //
    // `catalog_sub_hashes` indexed sixteen-bit halves of 32-bit landmark sub-hashes. The client
    // stopped producing those entirely — the table that held them was dropped on the device — so
    // the index had nothing left to point at, and there is no way to bucket a 128-dimension float
    // vector into sixteen-bit halves anyway.
    //
    // `mean` is stored beside the sequence because it is what candidates are filtered on. A full
    // comparison is quadratic in segment count, so most candidates have to be rejected without
    // reading their sequence at all, and the sequences are megabytes.
    //
    // `model` and `version` are part of the key in practice: a vector from a different embedder is
    // a different alphabet, and comparing across them would produce confident nonsense.
    "CREATE TABLE IF NOT EXISTS catalog_embeddings (
         recording_id TEXT PRIMARY KEY,
         embedding    BLOB NOT NULL,
         mean         BLOB NOT NULL,
         dim          INTEGER NOT NULL,
         segments     INTEGER NOT NULL,
         model        TEXT NOT NULL,
         version      INTEGER NOT NULL,
         updated_at   INTEGER NOT NULL
     );
     CREATE INDEX IF NOT EXISTS idx_catalog_embeddings_model
         ON catalog_embeddings(model, version);
     DROP TABLE IF EXISTS catalog_sub_hashes;
     ALTER TABLE catalog_recordings DROP COLUMN sub_hashes;",
    "ALTER TABLE catalog_recordings ADD COLUMN lyrics TEXT;",
    "ALTER TABLE catalog_recordings ADD COLUMN lyrics_source TEXT;",
    // 47 — artists become rows, so that something can be subscribed to.
    //
    // `catalog_recordings.artist` is free text copied off whatever tagged the file. There is no
    // identity in it: "Tyler, The Creator" and "tyler the creator" are two strings and the same
    // person, and a subscription keyed on either one misses every release filed under the other.
    //
    // `norm_name` is the identity and carries the UNIQUE. It holds the same normalisation
    // `norm::normalize_artist` applies — case folded, punctuation and articles stripped — which is
    // already what the library matcher uses to decide two tags mean one artist. `display_name` is
    // the first spelling seen, kept only to have something to print.
    //
    // The backfill below is deliberately done in SQL rather than by reading every row into Rust:
    // it runs inside the migration's transaction, so a database either gains the whole artist
    // table or none of it. It can only apply the cheap half of the normalisation — lowercase and
    // trim — because SQLite has no access to `normalize_artist`. That is the conservative
    // direction to be wrong in: two spellings that the Rust normaliser would have merged stay
    // separate rows until one of them is published again, and `upsert_artist` merges them then.
    // The opposite mistake, collapsing two artists who are not the same, cannot be undone.
    "CREATE TABLE IF NOT EXISTS artists (
         artist_id    TEXT PRIMARY KEY,
         norm_name    TEXT NOT NULL UNIQUE,
         display_name TEXT NOT NULL,
         external_id  TEXT,
         created_at   INTEGER NOT NULL
     );
     CREATE INDEX IF NOT EXISTS idx_artists_external ON artists(external_id);

     ALTER TABLE catalog_recordings ADD COLUMN artist_id TEXT;

     INSERT OR IGNORE INTO artists (artist_id, norm_name, display_name, external_id, created_at)
     SELECT
         lower(hex(randomblob(16))),
         lower(trim(artist)),
         min(artist),
         NULL,
         strftime('%s', 'now')
     FROM catalog_recordings
     WHERE artist IS NOT NULL AND trim(artist) <> ''
     GROUP BY lower(trim(artist));

     UPDATE catalog_recordings
        SET artist_id = (
            SELECT a.artist_id FROM artists a WHERE a.norm_name = lower(trim(catalog_recordings.artist))
        )
      WHERE artist IS NOT NULL AND trim(artist) <> '';

     CREATE INDEX IF NOT EXISTS idx_catalog_recordings_artist
         ON catalog_recordings(artist_id, updated_at);

     CREATE TABLE IF NOT EXISTS artist_subscriptions (
         user_id    TEXT NOT NULL,
         artist_id  TEXT NOT NULL,
         since_at   INTEGER NOT NULL,
         PRIMARY KEY (user_id, artist_id)
     );
     CREATE INDEX IF NOT EXISTS idx_artist_subscriptions_artist
         ON artist_subscriptions(artist_id);",
    // 48 — opting in to Popular on Agro.
    //
    // Defaults to 1, unlike every other visibility column here, and that is a deliberate
    // departure from the "defaults closed" rule the rest of this file follows: `popularity_counters`
    // never carries an account id and never exposes a row below the exposure floor, so this switch
    // controls disclosure of aggregate taste, not identity. There is nothing under it to leak.
    "ALTER TABLE users ADD COLUMN popular_opt_in INTEGER NOT NULL DEFAULT 1;",
    // 49 — a playlist can be shared with friends only.
    //
    // `is_public` stays the answer to "can every account open this"; this is the step between that
    // and owner-only. Defaults to 0, so every playlist that existed keeps exactly the audience it
    // had, and a new one is closed until someone opens it — see `playlist_visibility`.
    "ALTER TABLE playlists ADD COLUMN friends_only INTEGER NOT NULL DEFAULT 0;",
];
