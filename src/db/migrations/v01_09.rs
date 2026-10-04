//! Migrations 1 to 9. See [`super::MIGRATIONS`].

pub(super) const ENTRIES: &[&str] = &[
    // 1 — the music library.
    //
    // `music_tracks` and `jam_tracks` are dropped rather than reused: both were created by
    // `init_schema` and never read or written by anything, and `music_tracks` lacked every column
    // this needs (no owning device, no content hash, no size, no format).
    //
    // Note on `user_id`: it holds a **username**, matching `registered_nodes`, `handoff_state` and
    // `synced_settings`. Only `app_passwords.user_id` holds the `users.id` UUID. That split is
    // pre-existing and easy to trip over.
    "
    DROP TABLE IF EXISTS music_tracks;
    DROP TABLE IF EXISTS jam_tracks;

    -- One row per distinct *file*, identified by the SHA-256 of its bytes.
    CREATE TABLE IF NOT EXISTS library_tracks (
        content_hash   TEXT PRIMARY KEY,
        title          TEXT NOT NULL,
        artist         TEXT NOT NULL,
        album          TEXT,
        album_artist   TEXT,
        track_no       INTEGER,
        disc_no        INTEGER,
        year           INTEGER,
        genre          TEXT,
        duration_ms    INTEGER NOT NULL,
        size_bytes     INTEGER NOT NULL,
        format         TEXT,
        bitrate_kbps   INTEGER,
        -- Normalised for fuzzy matching; see `norm`. Stored rather than computed per query so the
        -- index below can be used.
        norm_artist    TEXT NOT NULL,
        norm_title     TEXT NOT NULL,
        -- Relative to AGRO_LIBRARY_ROOT. NULL when the server holds only the index entry and not
        -- the bytes, which is the whole of index-only mode.
        archived_path  TEXT,
        first_seen_at  TEXT NOT NULL,
        updated_at     TEXT NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_library_match
        ON library_tracks(norm_artist, norm_title);

    -- Which devices hold which file. The diff reads this.
    CREATE TABLE IF NOT EXISTS device_holdings (
        device_id    TEXT NOT NULL,
        user_id      TEXT NOT NULL,
        content_hash TEXT NOT NULL,
        -- Opaque client handle (a content URI, a filesystem path). Never interpreted here — it
        -- means something only on the device that reported it.
        local_ref    TEXT,
        reported_at  TEXT NOT NULL,
        PRIMARY KEY (device_id, content_hash)
    );
    CREATE INDEX IF NOT EXISTS idx_holdings_user
        ON device_holdings(user_id, content_hash);

    -- In-flight uploads, so an interrupted transfer resumes instead of restarting.
    CREATE TABLE IF NOT EXISTS upload_sessions (
        upload_id      TEXT PRIMARY KEY,
        user_id        TEXT NOT NULL,
        device_id      TEXT NOT NULL,
        content_hash   TEXT NOT NULL,
        size_bytes     INTEGER NOT NULL,
        received_bytes INTEGER NOT NULL DEFAULT 0,
        target         TEXT NOT NULL,
        created_at     TEXT NOT NULL,
        expires_at     TEXT NOT NULL
    );

    -- Files staged for a peer to collect. Size-capped and TTL'd: this host has a few GB of disk.
    CREATE TABLE IF NOT EXISTS spool_items (
        content_hash TEXT PRIMARY KEY,
        size_bytes   INTEGER NOT NULL,
        from_device  TEXT NOT NULL,
        user_id      TEXT NOT NULL,
        created_at   TEXT NOT NULL,
        expires_at   TEXT NOT NULL
    );
    ",
    // 2 — the performance variants of a title, sorted and comma-joined ("", "live",
    // "acoustic,live").
    //
    // Migration 1 stored only the normalised artist and title, and `normalize_title` strips
    // variant markers — so "Come As You Are" and "Come As You Are (Live)" were indistinguishable
    // in the index, and owning the studio cut suppressed the offer of the live take. Matching on
    // this column too is what keeps two genuinely different performances apart.
    //
    // Existing rows get '' and are corrected the next time their device reports them; the column
    // cannot be backfilled in SQL because the normalisation lives in Rust.
    "
    ALTER TABLE library_tracks ADD COLUMN norm_variants TEXT NOT NULL DEFAULT '';
    DROP INDEX IF EXISTS idx_library_match;
    CREATE INDEX idx_library_match
        ON library_tracks(norm_artist, norm_title, norm_variants);
    ",
    // 3 — the file extension the client declared, carried with the upload session.
    //
    // It used to live in an in-memory map keyed by upload id, which meant a server restart
    // mid-transfer lost it: the resumed upload then had no declared extension and fell back to
    // whatever lofty could infer, filing a FLAC as `.bin`. An upload that survives a restart has
    // to carry everything needed to finish it.
    "ALTER TABLE upload_sessions ADD COLUMN extension TEXT;",
    // 4 — share-link forwarding: the domain a user's players send share links out on, the hosts
    // this server will forward such a link to, and whether the whole thing is on.
    //
    // Deliberately not encrypted, unlike `server_url` beside it. `/listen` is a public route with
    // no user in context and so no passphrase to decrypt with — and none of the three is a secret.
    // The domain is printed in every link, and the host list *is* the allowlist: the thing that
    // decides where a stranger's click may go, which the server has to be able to read on its own.
    "
    ALTER TABLE synced_settings ADD COLUMN share_domain TEXT;
    ALTER TABLE synced_settings ADD COLUMN share_hosts TEXT;
    ALTER TABLE synced_settings ADD COLUMN share_enabled BOOLEAN DEFAULT 0;
    ",
    // 5 — UID-based short links for forwarding.
    "
    CREATE TABLE IF NOT EXISTS short_links (
        id TEXT PRIMARY KEY,
        target_url TEXT NOT NULL,
        user_id TEXT,
        created_at INTEGER NOT NULL,
        expires_at INTEGER
    );
    CREATE INDEX IF NOT EXISTS idx_short_links_created ON short_links(created_at);
    ",
    // 6 — click counts, so a link can be managed rather than only minted.
    //
    // A bare counter and a timestamp, nothing else. `/listen` deliberately records nothing about
    // who clicked (see `listen.rs`) and that does not change here: an aggregate that cannot
    // distinguish one visitor from another lets the owner see that a link is being used without
    // building a log of the people using it. No IP, no user agent, no referer.
    //
    // `source` says where the link came from, which is what decides whether deleting it also has
    // to reach a Navidrome server or is purely local to Agro.
    "
    ALTER TABLE short_links ADD COLUMN click_count INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE short_links ADD COLUMN last_clicked_at INTEGER;
    ALTER TABLE short_links ADD COLUMN source TEXT;
    ALTER TABLE ephemeral_shares ADD COLUMN click_count INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE ephemeral_shares ADD COLUMN last_clicked_at INTEGER;
    ",
    // 7 — album cover art, extracted from the files as they are archived.
    //
    // Only the *fact* of a cover lives here; the bytes go on disk under the library root, because
    // a few hundred JPEGs in SQLite would bloat every backup of a database that is otherwise tiny.
    // `album_key` is a hash of the album artist and album name (see `album_key` in `library.rs`),
    // which is also the on-disk filename — so nothing a tag contains ever reaches a path.
    //
    // Agro stored no artwork at all before this. The library was a list of hashes and strings,
    // which is enough to sync files and nothing like enough to *look* at a library.
    "
    CREATE TABLE IF NOT EXISTS library_covers (
        album_key    TEXT PRIMARY KEY,
        album_artist TEXT,
        album        TEXT,
        extension    TEXT NOT NULL,
        updated_at   INTEGER NOT NULL
    );
    ",
    // 8 — listening history that is actually written to.
    //
    // The `scrobbles` table has existed since the first schema and nothing ever inserted into it:
    // there was a writer function with no callers and a `agroRewind` resolver returning invented
    // Daft Punk figures. Clients now post their play history here, which is what makes one set of
    // statistics across every device possible at all.
    //
    // `client_type` distinguishes phone from desktop for per-device breakdowns. The unique index is
    // what makes ingest idempotent: a client's outbox retries after a failed upload, and without it
    // a flaky connection inflates every number it touches.
    "
    ALTER TABLE scrobbles ADD COLUMN client_type TEXT;
    CREATE UNIQUE INDEX IF NOT EXISTS idx_scrobbles_unique
        ON scrobbles(user_id, artist_name, track_title, played_at);
    CREATE INDEX IF NOT EXISTS idx_scrobbles_user_time ON scrobbles(user_id, played_at);
    ",
    // 9 — one admin, and guests who cannot reach past themselves.
    //
    // Every account used to be identical in power, which was correct for one household on a LAN
    // and is not correct for a server on the public internet. Three facts about an account now
    // exist that did not:
    //
    // - `role`: exactly one account owns the deployment. The oldest account is promoted here,
    //   because on an existing database that is the operator by construction.
    // - `state`: an account can exist without being allowed to do anything, which is what makes an
    //   approval queue a real gate rather than a label.
    // - `quota_bytes`: how much spool a guest may occupy. `0` means unlimited, which is what the
    //   admin gets — a cap on the person who owns the disk would be theatre.
    //
    // `passphrase_hash` and the token columns start empty and are filled in by
    // `migrate_credentials` at startup, because hashing cannot be done in SQL. Empty hashes never
    // verify, so every credential minted under the old plaintext scheme is dead the moment this
    // runs — which is the intended clean break.
    "
    ALTER TABLE users ADD COLUMN passphrase_hash TEXT NOT NULL DEFAULT '';
    ALTER TABLE users ADD COLUMN role TEXT NOT NULL DEFAULT 'member';
    ALTER TABLE users ADD COLUMN state TEXT NOT NULL DEFAULT 'active';
    ALTER TABLE users ADD COLUMN quota_bytes INTEGER NOT NULL DEFAULT 10485760;

    UPDATE users SET role = 'admin', quota_bytes = 0
     WHERE id = (SELECT id FROM users ORDER BY created_at ASC LIMIT 1);

    -- The settings fields in `synced_settings` are encrypted with the account passphrase. That
    -- passphrase is now an Argon2 hash and cannot be read back, so the key moves to its own
    -- column. Existing rows keep decrypting because the old plaintext `api_key` *was* that key.
    ALTER TABLE users ADD COLUMN settings_key TEXT NOT NULL DEFAULT '';
    UPDATE users SET settings_key = api_key WHERE settings_key = '';

    ALTER TABLE app_passwords ADD COLUMN token_prefix TEXT NOT NULL DEFAULT '';
    ALTER TABLE app_passwords ADD COLUMN token_hash TEXT NOT NULL DEFAULT '';
    CREATE INDEX IF NOT EXISTS idx_app_passwords_prefix ON app_passwords(token_prefix);
    ",
];
