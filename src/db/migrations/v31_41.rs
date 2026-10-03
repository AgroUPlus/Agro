//! Migrations 31 to 41. See [`super::MIGRATIONS`].

pub(super) const ENTRIES: &[&str] = &[
    // 31 — a second factor.
    //
    // Two columns rather than one, because a secret that has been generated is not a second factor
    // until someone has proved they can read codes from it. `totp_secret_enc` is written when
    // enrolment *starts*; `totp_confirmed_at` is written when the user proves it works. Until then
    // the secret is ignored entirely — a half-finished enrolment must not be able to lock anyone
    // out of their own account.
    //
    // The secret is encrypted at rest (see `totp::seal`). Unlike the settings vault, the server has
    // to be able to read this one — it is what verifies the code — so it cannot be client-sealed.
    // Encrypting it under a key held outside the database means a stolen `agro_data.db` on its own
    // does not yield working second factors.
    //
    // `totp_last_step` is the replay guard: a code is valid for a whole 30-second window, so
    // without recording the step it was accepted for, a code read over someone's shoulder can be
    // used again inside that window.
    "ALTER TABLE users ADD COLUMN totp_secret_enc TEXT;
     ALTER TABLE users ADD COLUMN totp_confirmed_at TEXT;
     ALTER TABLE users ADD COLUMN totp_last_step INTEGER;
     CREATE TABLE totp_recovery_codes (
        user_id TEXT NOT NULL,
        code_hash TEXT NOT NULL,
        created_at TEXT NOT NULL,
        used_at TEXT,
        PRIMARY KEY (user_id, code_hash)
     );
    ",
    // 32 — per-profile public key and E2EE encrypted track drops.
    //
    // `public_key` on users allows clients to publish an X25519 identity key. Senders can seal
    // drop messages and notes directly to the recipient's public key with zero server knowledge.
    // `note_ciphertext` and `is_encrypted` hold the sealed ciphertext payload for end-to-end encryption.
    "ALTER TABLE users ADD COLUMN public_key TEXT;
     ALTER TABLE track_drops ADD COLUMN note_ciphertext TEXT;
     ALTER TABLE track_drops ADD COLUMN is_encrypted INTEGER NOT NULL DEFAULT 0;
    ",
    // 33 — federated (OIDC) identities.
    //
    // `(issuer, subject)` is the primary key and the *only* join key. An identity provider's
    // `email` or `preferred_username` is a display hint that the IdP's own admin can edit, so
    // matching on either would mean anyone who can change a claim can take over the account that
    // happens to share it. The subject claim is the one value an IdP promises is stable and unique.
    //
    // There is deliberately no unique constraint on `user_id`: one account may link identities from
    // more than one provider. There *is* one on `(issuer, subject)`, so a single identity cannot be
    // pointed at two accounts.
    "CREATE TABLE federated_identities (
        issuer TEXT NOT NULL,
        subject TEXT NOT NULL,
        user_id TEXT NOT NULL,
        linked_at TEXT NOT NULL,
        claims_snapshot TEXT,
        PRIMARY KEY (issuer, subject)
     );
     CREATE INDEX federated_identities_user ON federated_identities(user_id);
     -- An account created through OIDC gets a generated passphrase nobody is ever shown, so the
     -- column is not empty. This records whether its owner could actually use it, which a hash
     -- cannot answer -- and it is what stops `unlinkFederatedIdentity` removing the last way in.
     ALTER TABLE users ADD COLUMN passphrase_is_usable INTEGER NOT NULL DEFAULT 1;
    ",
    // 29 - Proxy caching table
    "CREATE TABLE IF NOT EXISTS proxy_cache (
        url TEXT PRIMARY KEY,
        headers TEXT NOT NULL,
        body BLOB NOT NULL,
        expires_at INTEGER NOT NULL
    );",
    // 30 - Universal source-agnostic playlists
    "CREATE TABLE IF NOT EXISTS playlists (
        id          TEXT PRIMARY KEY,
        user_id     TEXT NOT NULL,
        title       TEXT NOT NULL,
        description TEXT,
        is_public   INTEGER NOT NULL DEFAULT 0,
        created_at  TEXT NOT NULL,
        updated_at  TEXT NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_playlists_user ON playlists(user_id);
    CREATE INDEX IF NOT EXISTS idx_playlists_public ON playlists(is_public);

    CREATE TABLE IF NOT EXISTS playlist_items (
        id          TEXT PRIMARY KEY,
        playlist_id TEXT NOT NULL,
        position    INTEGER NOT NULL,
        title       TEXT NOT NULL,
        artist      TEXT NOT NULL,
        album       TEXT,
        duration_ms INTEGER,
        norm_artist TEXT NOT NULL,
        norm_title  TEXT NOT NULL,
        artwork_url TEXT,
        origin_uri  TEXT,
        FOREIGN KEY (playlist_id) REFERENCES playlists(id) ON DELETE CASCADE
    );
    CREATE INDEX IF NOT EXISTS idx_playlist_items_playlist ON playlist_items(playlist_id, position);
    ",
    // 34 — a handoff can name the file it is playing.
    //
    // Presence already travels on this row, and a listener following along needs to ask the host's
    // device for *these bytes* rather than for a title. The host's `track_uri` cannot serve: it
    // names a row in their Navidrome or a video in their YouTube session and means nothing on
    // anyone else's device.
    //
    // NULL for everything that is not a hashed local file, which is most of what plays. That is
    // the honest answer — without a file there is nothing to transfer — and it is what makes the
    // peer-to-peer and relay tiers degrade to a name match instead of failing.
    "ALTER TABLE handoff_state ADD COLUMN content_hash TEXT;",
    // 35 — a jam track can name the file behind it, and the device holding it.
    //
    // `added_by` already records *who* queued a track, which is most of the answer: the member who
    // put it in the room is the one who can hand it over. What was missing is which of their
    // devices, and which bytes — without both, a room member with no copy of a track has nothing
    // to ask for and falls back to matching by name, which is what Jam did for every track.
    //
    // Both NULL for anything queued from a streaming source, and for every row queued before this.
    "ALTER TABLE jam_tracks ADD COLUMN content_hash TEXT;
     ALTER TABLE jam_tracks ADD COLUMN added_by_device TEXT;",
    // The shared fingerprint catalogue.
    //
    // Every client already identifies its own recordings without this; the catalogue exists so
    // that what one device worked out is not worked out again by every other device, and so that
    // a source with poor tags inherits the metadata a source with good ones supplied for the same
    // audio. Nothing here is required for a client to function alone.
    //
    // `sub_hashes` is the fingerprint itself, four bytes per entry. `catalog_sub_hashes` indexes
    // sixteen-bit halves of those, because a single flipped bit changes a whole sub-hash and an
    // index on whole ones matches nothing once audio has been through a lossy encoder.
    "CREATE TABLE IF NOT EXISTS catalog_recordings (
         recording_id   TEXT PRIMARY KEY,
         sub_hashes     BLOB NOT NULL,
         duration_ms    INTEGER NOT NULL,
         title          TEXT,
         artist         TEXT,
         album          TEXT,
         updated_at     INTEGER NOT NULL
     );
     CREATE TABLE IF NOT EXISTS catalog_sub_hashes (
         half         INTEGER NOT NULL,
         recording_id TEXT NOT NULL,
         PRIMARY KEY (half, recording_id)
     );
     CREATE INDEX IF NOT EXISTS idx_catalog_sub_hashes_recording
         ON catalog_sub_hashes(recording_id);
     CREATE TABLE IF NOT EXISTS catalog_sources (
         source_uri   TEXT PRIMARY KEY,
         recording_id TEXT NOT NULL,
         updated_at   INTEGER NOT NULL
     );
     CREATE INDEX IF NOT EXISTS idx_catalog_sources_recording
         ON catalog_sources(recording_id);
     CREATE INDEX IF NOT EXISTS idx_catalog_recordings_updated
         ON catalog_recordings(updated_at);",
    // Blinded popularity counters.
    //
    // **There is no account column here, and there must never be one.** That is the whole design:
    // the table cannot answer "what did this person listen to" because it does not hold the
    // information, rather than because a query declines to ask. Every other privacy property below
    // is a refinement of that one.
    //
    // Keyed on the same normalised columns as `library_tracks`, from `norm.rs`, so a track counted
    // by one client and a track counted by another are one row without either of them agreeing on
    // spelling — and so `reindex_normalisation` keeps this consistent with the library index.
    //
    // `bucket_day` is a whole day since the epoch, UTC, and is the *only* time recorded. A play at
    // 03:12 and a play at 22:47 are indistinguishable once counted, which is what stops the table
    // reconstructing a routine.
    "CREATE TABLE IF NOT EXISTS popularity_counters (
         bucket_day    INTEGER NOT NULL,
         norm_artist   TEXT NOT NULL,
         norm_title    TEXT NOT NULL,
         norm_variants TEXT NOT NULL,
         title         TEXT NOT NULL,
         artist        TEXT NOT NULL,
         album         TEXT,
         count         INTEGER NOT NULL DEFAULT 0,
         PRIMARY KEY (bucket_day, norm_artist, norm_title, norm_variants)
     );
     CREATE INDEX IF NOT EXISTS idx_popularity_bucket ON popularity_counters(bucket_day);",
    // Crowd-averaged acoustic vectors, for "more like this".
    //
    // **No account column here either**, for the same structural reason as the counters above and
    // one of its own: what a person's library *contains* is at least as revealing as what they
    // play, and a submitter column would make this exactly that list.
    //
    // `version` is part of the key rather than a column beside it. Vectors measured under two
    // different definitions of brightness are not comparable, so they must never average together
    // or share a neighbour search; keying on it makes mixing them impossible rather than merely
    // discouraged.
    "CREATE TABLE IF NOT EXISTS acoustic_vectors (
         norm_artist   TEXT NOT NULL,
         norm_title    TEXT NOT NULL,
         norm_variants TEXT NOT NULL,
         version       INTEGER NOT NULL,
         title         TEXT NOT NULL,
         artist        TEXT NOT NULL,
         tempo         REAL NOT NULL,
         energy        REAL NOT NULL,
         brightness    REAL NOT NULL,
         danceability  REAL NOT NULL,
         key_x         REAL NOT NULL,
         key_y         REAL NOT NULL,
         observations  INTEGER NOT NULL DEFAULT 1,
         PRIMARY KEY (norm_artist, norm_title, norm_variants, version)
     );",
    // 36 — identity keys belong to a device, not to an account.
    //
    // `users.public_key` is a single column, so signing in on a second phone published that
    // phone's key over the first one's. From that moment every drop sealed to the account was
    // readable by the new device alone: the phone that was already there could no longer open its
    // own incoming messages, and nothing sealed to it could ever be opened on the new one. The
    // damage lands at the second sign-in and cannot be undone afterwards, because the ciphertexts
    // are already sealed to a key the other device has never held.
    //
    // Keyed `(user_id, device_id)` for the same reason `registered_nodes` is (migration 10):
    // device ids are chosen by the client, so two accounts can collide on one and the device id
    // alone must not be the key.
    //
    // `users.public_key` stays, and stays written, as the compatibility mirror for clients that
    // have not updated. Existing keys are carried into the registry under the device id `legacy`,
    // so an account that has only ever had one device keeps working without signing in again.
    //
    // `drop_note_ciphertexts` holds one sealed copy of a note per device key it was sealed to —
    // including the sender's own, which is what makes a note readable by the person who sent it.
    // `track_drops.note_ciphertext` likewise stays, holding the recipient's copy.
    "CREATE TABLE IF NOT EXISTS user_device_keys (
         user_id       TEXT NOT NULL,
         device_id     TEXT NOT NULL,
         public_key    TEXT NOT NULL,
         registered_at TEXT NOT NULL,
         PRIMARY KEY (user_id, device_id)
     );
     INSERT OR IGNORE INTO user_device_keys (user_id, device_id, public_key, registered_at)
         SELECT username, 'legacy', public_key, COALESCE(created_at, '')
           FROM users
          WHERE public_key IS NOT NULL AND TRIM(public_key) != '';
     CREATE TABLE IF NOT EXISTS drop_note_ciphertexts (
         drop_id    TEXT NOT NULL,
         device_id  TEXT NOT NULL,
         ciphertext TEXT NOT NULL,
         PRIMARY KEY (drop_id, device_id)
     );
     CREATE INDEX IF NOT EXISTS idx_drop_note_ciphertexts_drop
         ON drop_note_ciphertexts(drop_id);",
];
