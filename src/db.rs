use rusqlite::{params, Connection, OptionalExtension, Result};
use std::path::Path;
use std::sync::{Arc, Mutex};

/// Takes the database away from the group and the world.
///
/// It holds Argon2 passphrase hashes and the SHA-256 of every device token — the whole credential
/// store. The deployment this project documents makes that worse rather than better: the systemd
/// unit in the README sets `UMask=0002` and a shared `SupplementaryGroups`, so that a music
/// library can be written by two services. New files land group-writable, and the database is a
/// new file like any other.
///
/// The sidecars matter as much as the database. `-wal` holds recent writes in plaintext until it
/// is checkpointed, so a 0600 database beside a 0644 write-ahead log protects nothing.
///
/// Best effort: a failure here is reported and startup continues. Refusing to run because a chmod
/// failed would take a working server down over a filesystem that may not have modes at all.
#[cfg(unix)]
fn restrict_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;

    for suffix in ["", "-wal", "-shm"] {
        let target = if suffix.is_empty() {
            path.to_path_buf()
        } else {
            let mut name = path.as_os_str().to_os_string();
            name.push(suffix);
            std::path::PathBuf::from(name)
        };
        if !target.exists() {
            continue;
        }
        if let Err(error) = std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600))
        {
            eprintln!(
                "agro: could not restrict permissions on {}: {error}",
                target.display()
            );
        }
    }
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) {}

/// Schema changes, in order. **Append only** — an entry's index is its version number, so
/// reordering or removing one silently skips it on every database that has already run it.
const MIGRATIONS: &[&str] = &[
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
    // 10 — device ids belong to an account.
    //
    // `registered_nodes.device_id` was the primary key on its own, but device ids are chosen by
    // the client. Two accounts could therefore collide on one, and `registerNode` — plus the
    // implicit node upsert inside `updateHandoff` — would let a guest overwrite the admin's node
    // row, including the `current_track` and `ip_address` shown in the dashboard.
    //
    // SQLite cannot alter a primary key in place, so the table is rebuilt. Rows that collided
    // under the old key are already lost; this only stops it happening again.
    "
    CREATE TABLE registered_nodes_v2 (
        device_id     TEXT NOT NULL,
        user_id       TEXT NOT NULL,
        petname       TEXT NOT NULL,
        client_type   TEXT NOT NULL,
        ip_address    TEXT,
        version       TEXT,
        current_track TEXT,
        last_seen_at  TEXT NOT NULL,
        PRIMARY KEY (user_id, device_id)
    );
    INSERT OR IGNORE INTO registered_nodes_v2
        (device_id, user_id, petname, client_type, ip_address, version, current_track, last_seen_at)
        SELECT device_id, user_id, petname, client_type, ip_address, version, current_track, last_seen_at
          FROM registered_nodes;
    DROP TABLE registered_nodes;
    ALTER TABLE registered_nodes_v2 RENAME TO registered_nodes;
    CREATE INDEX IF NOT EXISTS idx_nodes_user ON registered_nodes(user_id);
    ",
    // 11 — invitations, and the queue an invited account waits in.
    "
    CREATE TABLE IF NOT EXISTS invites (
        code       TEXT PRIMARY KEY,
        created_by TEXT NOT NULL,
        created_at TEXT NOT NULL,
        expires_at TEXT,
        max_uses   INTEGER NOT NULL DEFAULT 1,
        used_count INTEGER NOT NULL DEFAULT 0,
        revoked    INTEGER NOT NULL DEFAULT 0
    );
    ",
    // 12 — friendships, and the visibility that gates what they reveal.
    //
    // Both columns default to 0. A privacy setting that defaults open has already leaked by the
    // time the user finds it.
    "
    CREATE TABLE IF NOT EXISTS friendships (
        user_id    TEXT NOT NULL,
        friend_id  TEXT NOT NULL,
        state      TEXT NOT NULL,
        created_at TEXT NOT NULL,
        PRIMARY KEY (user_id, friend_id)
    );
    CREATE INDEX IF NOT EXISTS idx_friendships_friend ON friendships(friend_id);

    ALTER TABLE users ADD COLUMN show_now_playing INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE users ADD COLUMN show_stats INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE users ADD COLUMN private_until TEXT;
    ",
    // 13 — the profile a friend actually sees, and who is tuned in to whom.
    //
    // `discoverable` joins the two visibility columns from 12 and defaults closed for the same
    // reason they do: being listed in a public directory is a thing to opt into, not out of.
    //
    // `listen_along` is keyed on the listener alone. You can be followed by many people and follow
    // at most one — a second row for the same listener is not a state worth representing, it is two
    // players fighting over one output.
    "
    ALTER TABLE users ADD COLUMN display_name TEXT;
    ALTER TABLE users ADD COLUMN bio TEXT;
    ALTER TABLE users ADD COLUMN avatar_url TEXT;
    ALTER TABLE users ADD COLUMN discoverable INTEGER NOT NULL DEFAULT 0;

    CREATE TABLE IF NOT EXISTS listen_along (
        listener_id TEXT PRIMARY KEY,
        host_id     TEXT NOT NULL,
        started_at  TEXT NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_listen_along_host ON listen_along(host_id);
    
    ",
    // 14 — a library is private until its owner says otherwise.
    //
    // `library_stats` and `library_browse` both scoped their results as "tracks this user holds
    // OR anything archived on the server", which meant every account saw the operator's whole
    // archive counted and listed as its own library. This adds the switch that makes sharing a
    // decision: off by default, and only ever consulted for someone who is already an accepted
    // friend.
    "
    ALTER TABLE users ADD COLUMN share_library INTEGER NOT NULL DEFAULT 0;
    ",
    // 15 — jam sessions: a queue several people build together.
    //
    // Distinct from listen-along, which mirrors one person's playback. A jam has no single source:
    // anyone in it can add, and in `democracy` mode the order is decided by votes rather than by
    // whoever added first. The creator is its host — the only member who can change the mode, drop
    // somebody else's track, or end it.
    //
    // `code` is the whole credential for joining, so it is unique and indexed. Votes are one per
    // person per track, enforced by the primary key rather than by a check that could be raced.
    "
    CREATE TABLE IF NOT EXISTS jams (
        id         TEXT PRIMARY KEY,
        code       TEXT NOT NULL UNIQUE,
        host       TEXT NOT NULL,
        mode       TEXT NOT NULL DEFAULT 'democracy',
        created_at TEXT NOT NULL,
        ended_at   TEXT
    );

    CREATE TABLE IF NOT EXISTS jam_members (
        jam_id    TEXT NOT NULL,
        username  TEXT NOT NULL,
        joined_at TEXT NOT NULL,
        PRIMARY KEY (jam_id, username)
    );

    CREATE TABLE IF NOT EXISTS jam_tracks (
        id          TEXT PRIMARY KEY,
        jam_id      TEXT NOT NULL,
        added_by    TEXT NOT NULL,
        track_uri   TEXT NOT NULL,
        title       TEXT NOT NULL,
        artist      TEXT NOT NULL,
        artwork_url TEXT,
        added_at    TEXT NOT NULL,
        played      INTEGER NOT NULL DEFAULT 0
    );
    CREATE INDEX IF NOT EXISTS idx_jam_tracks_jam ON jam_tracks(jam_id);

    CREATE TABLE IF NOT EXISTS jam_votes (
        jam_id   TEXT NOT NULL,
        track_id TEXT NOT NULL,
        username TEXT NOT NULL,
        PRIMARY KEY (track_id, username)
    );
    CREATE INDEX IF NOT EXISTS idx_jam_votes_track ON jam_votes(track_id);
    ",
    // 16 — the jam session becomes the server's, not the clients'.
    //
    // Two changes, both of which move a decision out of the apps:
    //
    // `state` replaces the `played` flag. A flag could say "done" but not "waiting for the room to
    // accept it", so in democracy mode a track had nowhere to sit between being suggested and being
    // queued — votes ended up *sorting* the queue instead of deciding what got into it.
    //
    // `now_playing_id` and `started_at` make the server the clock. Every device used to pick the
    // top of the queue and start it whenever it happened to resolve, so a room played the same
    // order at different times and nothing could say what was being heard *now*. With a start time
    // held here, everyone plays the same track from the same offset and someone joining late is
    // dropped in at the right place.
    "
    ALTER TABLE jam_tracks ADD COLUMN duration_ms INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE jam_tracks ADD COLUMN state TEXT NOT NULL DEFAULT 'queued';
    UPDATE jam_tracks SET state = 'played' WHERE played = 1;
    CREATE INDEX IF NOT EXISTS idx_jam_tracks_state ON jam_tracks(jam_id, state);

    ALTER TABLE jams ADD COLUMN now_playing_id TEXT;
    ALTER TABLE jams ADD COLUMN started_at TEXT;
    ",
    // 17 — skipping, and jams a friend can find.
    //
    // `visibility` is how a jam stops being a secret. A code is the whole credential for joining,
    // which is right for a room you invite people into by hand, but it means a friend cannot join
    // something you are happy for them to join without you sending them a string. `friends` opens
    // it to accepted friends only — never to the instance at large.
    //
    // `jam_skips` is one vote per person per track, keyed the same way approvals are. Skipping is
    // deliberately not the same act as approving: an approval decides what enters the queue and is
    // one-way, a skip decides that the thing playing *now* should stop, and dies with the track.
    "
    ALTER TABLE jams ADD COLUMN visibility TEXT NOT NULL DEFAULT 'code';

    CREATE TABLE IF NOT EXISTS jam_skips (
        jam_id   TEXT NOT NULL,
        track_id TEXT NOT NULL,
        username TEXT NOT NULL,
        PRIMARY KEY (track_id, username)
    );
    CREATE INDEX IF NOT EXISTS idx_jam_skips_track ON jam_skips(track_id);
    ",
    // 18 — songs handed to a friend, and the consent that lets a history be read.
    //
    // A drop is a message that happens to be about a track, so it stores the track by *description*
    // rather than by reference. There is no foreign key to `library_tracks` because the sender may
    // not have the file here at all — a drop from YouTube or a streaming backend is still a drop —
    // and a key that only sometimes resolves is worse than an honest copy of the metadata.
    // `content_hash` and `track_uri` ride along when the sender happens to have them, so a
    // recipient can be offered the file rather than only the name.
    //
    // Nothing here cascades on the sender. A song someone gave you is yours: unfriending them, or
    // their account being deleted, is not a reason to take it back out of your inbox.
    "
    CREATE TABLE IF NOT EXISTS track_drops (
        id           TEXT PRIMARY KEY,
        from_user    TEXT NOT NULL,
        to_user      TEXT NOT NULL,
        track_title  TEXT NOT NULL,
        artist_name  TEXT NOT NULL,
        album_name   TEXT,
        artwork_url  TEXT,
        content_hash TEXT,
        track_uri    TEXT,
        note         TEXT,
        created_at   TEXT NOT NULL,
        read_at      TEXT,
        archived     INTEGER NOT NULL DEFAULT 0
    );
    CREATE INDEX IF NOT EXISTS idx_drops_inbox
        ON track_drops(to_user, archived, created_at DESC);
    CREATE INDEX IF NOT EXISTS idx_drops_sent
        ON track_drops(from_user, created_at DESC);

    -- The activity feed gets its own switch rather than reusing `show_now_playing`. Letting
    -- someone see what you are playing at this moment and letting them read what you have been
    -- into for the last month are different consents, and the second is much the more revealing
    -- of the two: it is a history, and it keeps being true after the moment has passed. Defaults
    -- off, like every other switch on this account.
    ALTER TABLE users ADD COLUMN show_activity INTEGER NOT NULL DEFAULT 0;
    ",
    // 13 — one emoji back, and a short-lived code for adding a friend in person.
    //
    // `reaction` turns an inbox into a conversation. Unlike `read_at`, which is deliberately kept
    // from the sender because a read receipt is surveillance, a reaction is something the
    // recipient *chose* to send — so it travels both ways. One column, not a table: exactly one
    // reaction per drop, replaced when it changes, because a row of six emoji under a song is a
    // different feature and not this one.
    //
    // `friend_codes` is for adding someone standing next to you. The existing `invites` table
    // cannot serve: those create *accounts*, are minted by administrators, and last for hours.
    // This is minted by any account for itself, redeems into a friend edge and nothing else, and
    // expires in minutes — a code photographed off a screen has to stop working before the person
    // who photographed it gets home. `used_at` makes it single-use: without it a code screenshotted
    // once could be redeemed by everyone it was ever shown to, for as long as it lived.
    "
    ALTER TABLE track_drops ADD COLUMN reaction TEXT;

    CREATE TABLE IF NOT EXISTS friend_codes (
        code       TEXT PRIMARY KEY,
        user_id    TEXT NOT NULL,
        created_at TEXT NOT NULL,
        expires_at TEXT NOT NULL,
        used_at    TEXT
    );
    CREATE INDEX IF NOT EXISTS idx_friend_codes_user ON friend_codes(user_id);
    CREATE INDEX IF NOT EXISTS idx_friend_codes_expiry ON friend_codes(expires_at);
    ",
    // 14 — a jam track that is a livestream rather than a recording.
    //
    // The clock has to tell "endless on purpose" from "we could not read a duration". Both arrive
    // as `duration_ms = 0`, and they want opposite treatment: an unmeasured recording gets a lease
    // so it cannot park the room forever, while a radio is *supposed* to keep playing until
    // somebody skips it.
    //
    // Defaults to 0, so everything already queued keeps the lease behaviour it was added under.
    "ALTER TABLE jam_tracks ADD COLUMN is_live INTEGER NOT NULL DEFAULT 0;",
    // 15 — incognito, held by the account rather than by a device.
    "ALTER TABLE users ADD COLUMN incognito INTEGER NOT NULL DEFAULT 0;",
    // 22 — can_archive permission and LAN addresses for direct P2P sync.
    "
    ALTER TABLE users ADD COLUMN can_archive INTEGER NOT NULL DEFAULT 0;
    UPDATE users SET can_archive = 1 WHERE role = 'admin';
    ALTER TABLE registered_nodes ADD COLUMN lan_address TEXT;
    ",
    // 23 — a handoff belongs to the device that reported it, not to the account.
    //
    // `handoff_state` was keyed on `user_id` alone, so the account held exactly one row and every
    // device overwrote it. That is right for "where was I", which is one answer per person, and
    // wrong for every other question asked of it — because the answer depends on *who is asking*.
    //
    // It is what stopped the desktop client proxying the phone's track to Discord. Pausing on the
    // desktop wrote the desktop's own paused state over the phone's playing one, and the client
    // filters out its own device, so the fleet's session vanished at the exact moment it became
    // the interesting one.
    //
    // Rows are keyed per device now and read back most-recent-first, so `playbackHandoff` answers
    // exactly as it did while also being able to answer "what is anyone *else* playing".
    "
    CREATE TABLE handoff_state_v2 (
        user_id      TEXT NOT NULL,
        device_id    TEXT NOT NULL,
        track_uri    TEXT NOT NULL,
        track_title  TEXT NOT NULL,
        artist_name  TEXT NOT NULL,
        album_name   TEXT,
        artwork_url  TEXT,
        position_ms  INTEGER NOT NULL,
        is_playing   BOOLEAN NOT NULL,
        updated_at   TEXT NOT NULL,
        queue_json   TEXT,
        queue_index  INTEGER,
        PRIMARY KEY (user_id, device_id)
    );
    INSERT OR IGNORE INTO handoff_state_v2
        (user_id, device_id, track_uri, track_title, artist_name, album_name, artwork_url,
         position_ms, is_playing, updated_at, queue_json, queue_index)
        SELECT user_id, device_id, track_uri, track_title, artist_name, album_name, artwork_url,
               position_ms, is_playing, updated_at, queue_json, queue_index
          FROM handoff_state;
    DROP TABLE handoff_state;
    ALTER TABLE handoff_state_v2 RENAME TO handoff_state;
    CREATE INDEX IF NOT EXISTS idx_handoff_user ON handoff_state(user_id, updated_at DESC);
    ",
    // 24 — how long the track is, alongside where in it the sender was.
    //
    // A handoff carried a position and nothing to measure it against, so anything rendering one
    // could only show an elapsed count: a progress bar needs both ends. Every sender already knows
    // the length — it is the player telling us — so this is a field that was simply never asked
    // for rather than one that has to be worked out.
    //
    // Zero means "the sender did not say", which is also what a livestream reports, and both want
    // the same treatment: no bar, just a running clock.
    "ALTER TABLE handoff_state ADD COLUMN duration_ms INTEGER NOT NULL DEFAULT 0;",
];

#[derive(Clone)]
pub struct Db {
    /// `pub(crate)` so the library index can keep its own `impl Db` block in `db_library`, rather
    /// than growing this file by another few hundred lines of unrelated SQL.
    pub(crate) conn: Arc<Mutex<Connection>>,
}

impl Db {
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let conn = Connection::open(&path)?;
        let db = Db {
            conn: Arc::new(Mutex::new(conn)),
        };
        db.init_schema()?;
        db.migrate()?;
        // After the schema, so the -wal and -shm SQLite creates along the way are covered too.
        restrict_permissions(&path);
        Ok(db)
    }

    pub fn new_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let db = Db {
            conn: Arc::new(Mutex::new(conn)),
        };
        db.init_schema()?;
        db.migrate()?;
        Ok(db)
    }

    /// Brings the database up to date.
    ///
    /// Two mechanisms, for two eras. [`Self::migrate_handoff_queue`] predates any version stamp
    /// and stays idempotent because databases exist in both states. Everything since is a numbered
    /// entry in [`MIGRATIONS`], applied in order, each in its own transaction, with
    /// `PRAGMA user_version` stamped as it goes — so each runs exactly once and a failure aborts
    /// startup rather than leaving a half-migrated database serving requests.
    fn migrate(&self) -> Result<()> {
        self.migrate_handoff_queue();

        let mut conn = self.conn.lock().unwrap();
        let current: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;

        for (index, migration) in MIGRATIONS.iter().enumerate() {
            let version = index as i64 + 1;
            if version <= current {
                continue;
            }
            let tx = conn.transaction()?;
            tx.execute_batch(migration)?;
            // PRAGMA takes no bound parameters, and `version` is a loop index over a compile-time
            // constant rather than anything a caller supplied.
            tx.execute_batch(&format!("PRAGMA user_version = {version}"))?;
            tx.commit()?;
        }
        Ok(())
    }

    /// Adds the queue columns to a database created before they existed.
    ///
    /// `init_schema` includes them now, so this only ever does anything on a database that
    /// predates them. SQLite has no `ADD COLUMN IF NOT EXISTS`, and the only failure mode is
    /// "already there", so the error is the expected outcome on every run after the first — which
    /// is exactly why nothing newer than this is done that way.
    fn migrate_handoff_queue(&self) {
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute("ALTER TABLE handoff_state ADD COLUMN queue_json TEXT", []);
        let _ = conn.execute("ALTER TABLE handoff_state ADD COLUMN queue_index INTEGER", []);
    }

    fn init_schema(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS users (
                id TEXT PRIMARY KEY,
                username TEXT UNIQUE NOT NULL,
                api_key TEXT NOT NULL,
                created_at TEXT NOT NULL
            );

            -- Per-client credentials. A device gets its own token so it can be revoked on its
            -- own, without rotating the account passphrase every other device is using.
            CREATE TABLE IF NOT EXISTS app_passwords (
                token TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                label TEXT NOT NULL,
                created_at TEXT NOT NULL,
                last_used_at TEXT
            );

            CREATE TABLE IF NOT EXISTS plugins_state (
                id TEXT PRIMARY KEY,
                is_enabled BOOLEAN NOT NULL
            );

            CREATE TABLE IF NOT EXISTS scrobbles (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id TEXT NOT NULL,
                track_title TEXT NOT NULL,
                artist_name TEXT NOT NULL,
                album_name TEXT,
                genre TEXT,
                duration_secs INTEGER NOT NULL,
                device_name TEXT NOT NULL,
                played_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS handoff_state (
                user_id TEXT PRIMARY KEY,
                track_uri TEXT NOT NULL,
                track_title TEXT NOT NULL,
                artist_name TEXT NOT NULL,
                album_name TEXT,
                artwork_url TEXT,
                position_ms INTEGER NOT NULL,
                is_playing BOOLEAN NOT NULL,
                device_id TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                queue_json TEXT,
                queue_index INTEGER
            );

            CREATE TABLE IF NOT EXISTS ephemeral_shares (
                token TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                track_title TEXT NOT NULL,
                artist_name TEXT NOT NULL,
                album_name TEXT,
                audio_url TEXT NOT NULL,
                expires_at TEXT NOT NULL
            );

            -- `music_tracks` and `jam_tracks` used to be created here and were never read or
            -- written by anything. They are dropped by migration 1; the real library index is
            -- `library_tracks`.

            CREATE TABLE IF NOT EXISTS registered_nodes (
                device_id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                petname TEXT NOT NULL,
                client_type TEXT NOT NULL,
                ip_address TEXT,
                version TEXT,
                current_track TEXT,
                last_seen_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS synced_settings (
                user_id TEXT PRIMARY KEY,
                server_url TEXT,
                server_username TEXT,
                lrclib_url TEXT,
                lyrics_fetch_online BOOLEAN DEFAULT 1,
                stream_format TEXT DEFAULT 'FLAC',
                -- The share-link columns are added by migration 4, not here: a fresh database
                -- runs `init_schema` and then *every* migration, so a column declared in both
                -- places aborts startup on 'duplicate column name'.
                updated_at TEXT NOT NULL
            );

            -- The queue a session was playing, as a JSON array. Added after the table shipped,
            -- so existing databases pick it up through the guarded ALTER in `migrate_queue`.

            -- Clean up any test dummy nodes
            DELETE FROM registered_nodes WHERE device_id IN ('wander-workstation', 'wanda-pixel8');
            ",
        )?;
        Ok(())
    }

    pub fn create_user(&self, username: &str, api_key: &str) -> Result<String> {
        let conn = self.conn.lock().unwrap();
        let user_id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO users (id, username, api_key, created_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(username) DO UPDATE SET api_key = excluded.api_key",
            params![user_id, username, api_key, now],
        )?;
        Ok(user_id)
    }

    pub fn get_or_create_user(&self, username: &str, preferred_passphrase: Option<&str>) -> Result<(String, String)> {
        if let Some((id, _, key)) = self.get_user_by_username(username)? {
            return Ok((id, key));
        }
        let passphrase = preferred_passphrase
            .filter(|p| !p.trim().is_empty())
            .map(String::from)
            .unwrap_or_else(crate::passphrase::generate_passphrase);
        let user_id = self.create_user(username, &passphrase)?;
        Ok((user_id, passphrase))
    }

    /// Removes an account and everything that belongs to it. Deliberately thorough: leaving a
    /// user's nodes, session and settings behind would let a recreated account inherit them.
    pub fn delete_user(&self, username: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let user_id: Option<String> = conn
            .query_row(
                "SELECT id FROM users WHERE username = ?1",
                params![username],
                |row| row.get(0),
            )
            .optional()?;
        let Some(user_id) = user_id else {
            return Ok(false);
        };
        conn.execute("DELETE FROM app_passwords WHERE user_id = ?1", params![user_id])?;
        conn.execute("DELETE FROM registered_nodes WHERE user_id = ?1", params![username])?;
        conn.execute("DELETE FROM handoff_state WHERE user_id = ?1", params![username])?;
        conn.execute("DELETE FROM synced_settings WHERE user_id = ?1", params![username])?;
        conn.execute("DELETE FROM users WHERE id = ?1", params![user_id])?;
        Ok(true)
    }

    pub fn list_users(&self) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT username FROM users ORDER BY created_at ASC")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        let mut users = Vec::new();
        for r in rows {
            users.push(r?);
        }
        if users.is_empty() {
            users.push("alpha".to_string());
        }
        Ok(users)
    }

    pub fn authenticate_user(&self, username: &str, passphrase: &str) -> Result<bool> {
        if username.trim().is_empty() || passphrase.trim().is_empty() {
            return Ok(false);
        }
        if let Some((_, _, stored_pass)) = self.get_user_by_username(username)? {
            Ok(stored_pass.trim() == passphrase.trim())
        } else {
            // Frictionless first-time auto-registration with provided passphrase
            let _ = self.create_user(username, passphrase)?;
            Ok(true)
        }
    }

    pub fn validate_api_key(&self, api_key: &str) -> Result<Option<(String, String)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id, username FROM users WHERE api_key = ?1")?;
        let mut rows = stmt.query(params![api_key])?;
        if let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            let username: String = row.get(1)?;
            Ok(Some((id, username)))
        } else {
            Ok(None)
        }
    }

    /// How many accounts exist. Zero means the server has never been set up, which is the only
    /// state in which an unauthenticated request is allowed to create one.
    pub fn user_count(&self) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT COUNT(*) FROM users", [], |row| row.get(0))
    }

    /// Resolves a bearer token to its username, accepting either the account passphrase or one of
    /// its app passwords. Returns None for anything else — including an empty token.
    pub fn user_for_token(&self, token: &str) -> Result<Option<String>> {
        if token.is_empty() {
            return Ok(None);
        }
        let conn = self.conn.lock().unwrap();
        let account: Option<String> = conn
            .query_row(
                "SELECT username FROM users WHERE api_key = ?1",
                params![token],
                |row| row.get(0),
            )
            .optional()?;
        if account.is_some() {
            return Ok(account);
        }

        let via_app_password: Option<String> = conn
            .query_row(
                "SELECT u.username FROM app_passwords a
                 JOIN users u ON u.id = a.user_id
                 WHERE a.token = ?1",
                params![token],
                |row| row.get(0),
            )
            .optional()?;
        if via_app_password.is_some() {
            let now = chrono::Utc::now().to_rfc3339();
            let _ = conn.execute(
                "UPDATE app_passwords SET last_used_at = ?1 WHERE token = ?2",
                params![now, token],
            );
        }
        Ok(via_app_password)
    }

    pub fn create_app_password(&self, username: &str, label: &str, token: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let user_id: String = conn.query_row(
            "SELECT id FROM users WHERE username = ?1",
            params![username],
            |row| row.get(0),
        )?;
        conn.execute(
            "INSERT INTO app_passwords (token, user_id, label, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![token, user_id, label, chrono::Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    /// Never returns the token itself: a credential is shown once, at creation.
    pub fn list_app_passwords(&self, username: &str) -> Result<Vec<AppPasswordRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT a.rowid, a.label, a.created_at, a.last_used_at FROM app_passwords a
             JOIN users u ON u.id = a.user_id
             WHERE u.username = ?1 COLLATE NOCASE ORDER BY a.created_at DESC",
        )?;
        let rows = stmt.query_map(params![username.trim()], |row| {
            Ok(AppPasswordRecord {
                id: row.get(0)?,
                label: row.get(1)?,
                created_at: row.get(2)?,
                last_used_at: row.get(3)?,
            })
        })?;
        rows.collect()
    }

    /// Revokes one credential, identified by the id [`list_app_passwords`] reported.
    ///
    /// Scoped to the account in the same statement rather than checked beforehand: an id is just a
    /// number, and a caller who guesses someone else's must not have it deleted for them.
    pub fn revoke_app_password(&self, username: &str, id: i64) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let removed = conn.execute(
            "DELETE FROM app_passwords WHERE rowid = ?1 AND user_id = (
                 SELECT id FROM users WHERE username = ?2 COLLATE NOCASE
             )",
            params![id, username.trim()],
        )?;
        Ok(removed > 0)
    }

    pub fn get_user_by_username(&self, username: &str) -> Result<Option<(String, String, String)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id, username, api_key FROM users WHERE username = ?1")?;
        let mut rows = stmt.query(params![username])?;
        if let Some(row) = rows.next()? {
            Ok(Some((row.get(0)?, row.get(1)?, row.get(2)?)))
        } else {
            Ok(None)
        }
    }

    pub fn set_plugin_enabled(&self, plugin_id: &str, is_enabled: bool) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO plugins_state (id, is_enabled) VALUES (?1, ?2)
             ON CONFLICT(id) DO UPDATE SET is_enabled = excluded.is_enabled",
            params![plugin_id, is_enabled],
        )?;
        Ok(())
    }

    pub fn get_plugin_states(&self) -> Result<std::collections::HashMap<String, bool>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id, is_enabled FROM plugins_state")?;
        let mut rows = stmt.query([])?;
        let mut map = std::collections::HashMap::new();
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            let enabled: bool = row.get(1)?;
            map.insert(id, enabled);
        }
        Ok(map)
    }

    pub fn update_handoff(
        &self,
        user_id: &str,
        track_uri: &str,
        track_title: &str,
        artist_name: &str,
        album_name: Option<&str>,
        artwork_url: Option<&str>,
        position_ms: i64,
        duration_ms: i64,
        is_playing: bool,
        device_id: &str,
        queue_json: Option<&str>,
        queue_index: Option<i64>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO handoff_state (user_id, track_uri, track_title, artist_name, album_name, artwork_url, position_ms, is_playing, device_id, updated_at, queue_json, queue_index, duration_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
             ON CONFLICT(user_id, device_id) DO UPDATE SET
             track_uri = excluded.track_uri,
             track_title = excluded.track_title,
             artist_name = excluded.artist_name,
             album_name = excluded.album_name,
             artwork_url = excluded.artwork_url,
             position_ms = excluded.position_ms,
             -- A sender that does not know the length must not erase one that did: a livestream
             -- and a length not measured yet both arrive as 0, and only one of them is an answer.
             duration_ms = CASE WHEN excluded.duration_ms > 0
                                THEN excluded.duration_ms
                                ELSE handoff_state.duration_ms END,
             is_playing = excluded.is_playing,
             updated_at = excluded.updated_at,
             -- A heartbeat that carries no queue must not erase the one already stored: only a
             -- client that actually sent a queue replaces it.
             queue_json = COALESCE(excluded.queue_json, handoff_state.queue_json),
             queue_index = COALESCE(excluded.queue_index, handoff_state.queue_index)",
            params![user_id, track_uri, track_title, artist_name, album_name, artwork_url, position_ms, is_playing, device_id, now, queue_json, queue_index, duration_ms],
        )?;
        Ok(())
    }

    /// Where the account left off — the most recent report from any of its devices.
    ///
    /// One row per device since migration 23, so "the account's handoff" is now a choice rather
    /// than the only row there is. This keeps the original answer: whatever happened last.
    pub fn get_handoff(&self, user_id: &str) -> Result<Option<HandoffRecord>> {
        self.latest_handoff(user_id, None)
    }

    /// The same, from any device *except* one.
    ///
    /// What a client asks when it is looking for the rest of the fleet rather than for itself: a
    /// desktop player proxying the phone's track to Discord has to be able to tell the two apart,
    /// and its own paused state is the one answer that is never useful to it.
    pub fn get_handoff_excluding(
        &self,
        user_id: &str,
        device_id: &str,
    ) -> Result<Option<HandoffRecord>> {
        self.latest_handoff(user_id, Some(device_id))
    }

    fn latest_handoff(
        &self,
        user_id: &str,
        exclude_device: Option<&str>,
    ) -> Result<Option<HandoffRecord>> {
        let conn = self.conn.lock().unwrap();
        // `updated_at` is an RFC 3339 stamp written by this process, always at the same offset, so
        // it orders lexicographically — no date parsing, which would fail silently to NULL and
        // scramble the order rather than erroring.
        let mut stmt = conn.prepare(
            "SELECT track_uri, track_title, artist_name, album_name, artwork_url, position_ms,
                    is_playing, device_id, updated_at, queue_json, queue_index, duration_ms
               FROM handoff_state
              WHERE user_id = ?1 AND (?2 IS NULL OR device_id != ?2)
              ORDER BY updated_at DESC
              LIMIT 1",
        )?;
        let mut rows = stmt.query(params![user_id, exclude_device])?;
        if let Some(row) = rows.next()? {
            Ok(Some(HandoffRecord {
                track_uri: row.get(0)?,
                track_title: row.get(1)?,
                artist_name: row.get(2)?,
                album_name: row.get(3)?,
                artwork_url: row.get(4)?,
                position_ms: row.get(5)?,
                is_playing: row.get(6)?,
                device_id: row.get(7)?,
                updated_at: row.get(8)?,
                queue_json: row.get(9)?,
                queue_index: row.get(10)?,
                duration_ms: row.get(11)?,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn record_scrobble(
        &self,
        user_id: &str,
        track_title: &str,
        artist_name: &str,
        album_name: Option<&str>,
        genre: Option<&str>,
        duration_secs: i64,
        device_name: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO scrobbles (user_id, track_title, artist_name, album_name, genre, duration_secs, device_name, played_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![user_id, track_title, artist_name, album_name, genre, duration_secs, device_name, now],
        )?;
        Ok(())
    }

    /// Ingests a batch of plays from one device.
    ///
    /// `INSERT OR IGNORE` against the unique index from migration 8, so a client re-sending an
    /// outbox it was not sure landed does not double every play in it. Returns how many rows were
    /// genuinely new, which is what lets a client tell "already had it" from "did not work".
    pub fn record_scrobbles(
        &self,
        user_id: &str,
        device_name: &str,
        client_type: Option<&str>,
        entries: &[ScrobbleEntry],
    ) -> Result<usize> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let mut inserted = 0;
        {
            let mut stmt = tx.prepare(
                "INSERT OR IGNORE INTO scrobbles
                     (user_id, track_title, artist_name, album_name, genre, duration_secs,
                      device_name, played_at, client_type)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?;
            for entry in entries {
                inserted += stmt.execute(params![
                    user_id,
                    entry.track_title,
                    entry.artist_name,
                    entry.album_name,
                    entry.genre,
                    entry.duration_secs,
                    device_name,
                    entry.played_at,
                    client_type,
                ])?;
            }
        }
        tx.commit()?;
        Ok(inserted)
    }

    /// Raw plays for an account, newest last.
    ///
    /// Deliberately returns rows rather than aggregates. The desktop client already computes a
    /// specific set of statistics from a local history file, and the numbers here have to agree
    /// with those exactly or switching a device between local and centralised stats looks like data
    /// loss. Sharing the *shape* of the computation is how that is guaranteed, so the aggregation
    /// lives in one place (`stats.rs`) rather than being re-expressed in SQL.
    pub fn scrobble_rows(
        &self,
        user_id: &str,
        device_name: Option<&str>,
        since: Option<&str>,
    ) -> Result<Vec<ScrobbleRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT s.track_title, s.artist_name, s.album_name, s.genre, s.duration_secs,
                    COALESCE(NULLIF(rn.petname, ''), s.device_name) AS device_name,
                    s.played_at
             FROM scrobbles s
             LEFT JOIN registered_nodes rn
                    ON rn.user_id = s.user_id COLLATE NOCASE
                   AND (rn.device_id = s.device_name OR rn.petname = s.device_name)
             WHERE s.user_id = ?1
               AND (?2 IS NULL OR s.device_name = ?2 OR rn.petname = ?2 OR rn.device_id = ?2)
               AND (?3 IS NULL OR s.played_at >= ?3)
             ORDER BY s.played_at",
        )?;
        let mut rows = stmt.query(params![user_id, device_name, since])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(ScrobbleRow {
                track_title: row.get(0)?,
                artist_name: row.get(1)?,
                album_name: row.get(2)?,
                genre: row.get(3)?,
                duration_secs: row.get(4)?,
                device_name: row.get(5)?,
                played_at: row.get(6)?,
            });
        }
        Ok(out)
    }

    pub fn create_ephemeral_share(
        &self,
        user_id: &str,
        track_title: &str,
        artist_name: &str,
        album_name: Option<&str>,
        audio_url: &str,
        ttl_hours: i64,
    ) -> Result<String> {
        let conn = self.conn.lock().unwrap();
        let token = uuid::Uuid::new_v4().to_string();
        let expires_at = (chrono::Utc::now() + chrono::Duration::hours(ttl_hours)).to_rfc3339();
        conn.execute(
            "INSERT INTO ephemeral_shares (token, user_id, track_title, artist_name, album_name, audio_url, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![token, user_id, track_title, artist_name, album_name, audio_url, expires_at],
        )?;
        Ok(token)
    }

    pub fn get_ephemeral_share(&self, token: &str) -> Result<Option<ShareRecord>> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        let mut stmt = conn.prepare("SELECT track_title, artist_name, album_name, audio_url, expires_at FROM ephemeral_shares WHERE token = ?1 AND expires_at > ?2")?;
        let mut rows = stmt.query(params![token, now])?;
        if let Some(row) = rows.next()? {
            Ok(Some(ShareRecord {
                track_title: row.get(0)?,
                artist_name: row.get(1)?,
                album_name: row.get(2)?,
                audio_url: row.get(3)?,
                expires_at: row.get(4)?,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn upsert_node(
        &self,
        device_id: &str,
        user_id: &str,
        petname: NodeName<'_>,
        client_type: &str,
        ip_address: Option<&str>,
        lan_address: Option<&str>,
        version: Option<&str>,
        current_track: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        // The name is decided in SQL rather than by reading the row first, so a heartbeat that
        // arrives while the user is renaming the device cannot write back the name it read.
        conn.execute(
            "INSERT INTO registered_nodes (device_id, user_id, petname, client_type, ip_address, lan_address, version, current_track, last_seen_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(user_id, device_id) DO UPDATE SET
             petname = CASE WHEN ?10 AND excluded.petname != '' THEN excluded.petname ELSE registered_nodes.petname END,
             client_type = excluded.client_type,
             ip_address = COALESCE(excluded.ip_address, registered_nodes.ip_address),
             lan_address = COALESCE(excluded.lan_address, registered_nodes.lan_address),
             version = COALESCE(excluded.version, registered_nodes.version),
             current_track = COALESCE(excluded.current_track, registered_nodes.current_track),
             last_seen_at = excluded.last_seen_at",
            params![
                device_id,
                user_id,
                petname.as_str(),
                client_type,
                ip_address,
                lan_address,
                version,
                current_track,
                now,
                petname.overwrites(),
            ],
        )?;
        Ok(())
    }

    /// Every registered node, across users. The plugin list needs a whole-deployment view rather
    /// than one user's devices.
    pub fn get_all_nodes(&self) -> Result<Vec<NodeRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT device_id, user_id, petname, client_type, ip_address, lan_address, version, current_track, last_seen_at
             FROM registered_nodes ORDER BY last_seen_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(NodeRecord {
                device_id: row.get(0)?,
                user_id: row.get(1)?,
                petname: row.get(2)?,
                client_type: row.get(3)?,
                ip_address: row.get(4)?,
                lan_address: row.get(5)?,
                version: row.get(6)?,
                current_track: row.get(7)?,
                last_seen_at: row.get(8)?,
            })
        })?;
        rows.collect()
    }

    /// Renames one device.
    ///
    /// The name is the only handle a person has on a device — `device_id` is opaque and the client
    /// picks it — so being stuck with an auto-generated one until the client happens to send a new
    /// name is a poor place to leave someone.
    pub fn rename_node(&self, user_id: &str, device_id: &str, petname: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let old_petname: Option<String> = conn
            .query_row(
                "SELECT petname FROM registered_nodes WHERE user_id = ?1 COLLATE NOCASE AND device_id = ?2",
                params![user_id.trim(), device_id.trim()],
                |row| row.get(0),
            )
            .ok();

        let changed = conn.execute(
            "UPDATE registered_nodes SET petname = ?3
              WHERE user_id = ?1 COLLATE NOCASE AND device_id = ?2",
            params![user_id.trim(), device_id.trim(), petname.trim()],
        )?;

        if let Some(old) = old_petname {
            if !old.is_empty() {
                let _ = conn.execute(
                    "UPDATE scrobbles SET device_name = ?3
                     WHERE user_id = ?1 COLLATE NOCASE AND (device_name = ?2 OR device_name = ?4)",
                    params![user_id.trim(), device_id.trim(), petname.trim(), old.trim()],
                );
            }
        }

        Ok(changed > 0)
    }

    pub fn get_active_nodes(&self, user_id: &str) -> Result<Vec<NodeRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT device_id, user_id, petname, client_type, ip_address, lan_address, version, current_track, last_seen_at
             FROM registered_nodes WHERE user_id = ?1 ORDER BY last_seen_at DESC"
        )?;
        let rows = stmt.query_map(params![user_id], |row| {
            Ok(NodeRecord {
                device_id: row.get(0)?,
                user_id: row.get(1)?,
                petname: row.get(2)?,
                client_type: row.get(3)?,
                ip_address: row.get(4)?,
                lan_address: row.get(5)?,
                version: row.get(6)?,
                current_track: row.get(7)?,
                last_seen_at: row.get(8)?,
            })
        })?;

        let mut nodes = Vec::new();
        for r in rows {
            nodes.push(r?);
        }
        Ok(nodes)
    }

    pub fn delete_node(&self, user_id: &str, device_id: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let affected = conn.execute(
            "DELETE FROM registered_nodes WHERE user_id = ?1 AND device_id = ?2",
            params![user_id, device_id],
        )?;
        Ok(affected > 0)
    }

    pub fn upsert_synced_settings(
        &self,
        user_id: &str,
        server_url: Option<&str>,
        server_username: Option<&str>,
        lrclib_url: Option<&str>,
        lyrics_fetch_online: Option<bool>,
        stream_format: Option<&str>,
        share: ShareSettingsInput<'_>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO synced_settings (user_id, server_url, server_username, lrclib_url, lyrics_fetch_online, stream_format, share_domain, share_hosts, share_enabled, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(user_id) DO UPDATE SET
             server_url = COALESCE(excluded.server_url, synced_settings.server_url),
             server_username = COALESCE(excluded.server_username, synced_settings.server_username),
             lrclib_url = COALESCE(excluded.lrclib_url, synced_settings.lrclib_url),
             lyrics_fetch_online = COALESCE(excluded.lyrics_fetch_online, synced_settings.lyrics_fetch_online),
             stream_format = COALESCE(excluded.stream_format, synced_settings.stream_format),
             share_domain = COALESCE(excluded.share_domain, synced_settings.share_domain),
             share_hosts = COALESCE(excluded.share_hosts, synced_settings.share_hosts),
             share_enabled = COALESCE(excluded.share_enabled, synced_settings.share_enabled),
             updated_at = excluded.updated_at",
            params![
                user_id,
                server_url,
                server_username,
                lrclib_url,
                lyrics_fetch_online,
                stream_format,
                share.domain,
                share.hosts,
                share.enabled,
                now
            ],
        )?;
        Ok(())
    }

    pub fn get_synced_settings(&self, user_id: &str) -> Result<Option<SyncedSettingsRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT server_url, server_username, lrclib_url, lyrics_fetch_online, stream_format,
                    share_domain, share_hosts, share_enabled, updated_at
             FROM synced_settings WHERE user_id = ?1"
        )?;
        let mut rows = stmt.query(params![user_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(SyncedSettingsRecord {
                server_url: row.get(0)?,
                server_username: row.get(1)?,
                lrclib_url: row.get(2)?,
                lyrics_fetch_online: row.get(3)?,
                stream_format: row.get(4)?,
                share_domain: row.get(5)?,
                share_hosts: row.get(6)?,
                share_enabled: row.get(7)?,
                updated_at: row.get(8)?,
            }))
        } else {
            Ok(None)
        }
    }

    /// Every host any account on this server has allowed, for the public `/listen` route.
    ///
    /// That route has no user in context — a shared link is opened by a stranger, with no token —
    /// so the allowlist it enforces is the union of what the accounts here have set. Only rows
    /// with forwarding actually switched on contribute to it.
    pub fn allowed_share_hosts(&self) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT share_hosts FROM synced_settings
             WHERE share_enabled = 1 AND share_hosts IS NOT NULL",
        )?;
        let mut rows = stmt.query([])?;
        let mut hosts: Vec<String> = Vec::new();
        while let Some(row) = rows.next()? {
            let raw: String = row.get(0)?;
            hosts.extend(
                raw.split(',')
                    .map(|host| host.trim().to_lowercase())
                    .filter(|host| !host.is_empty()),
            );
        }
        hosts.sort();
        hosts.dedup();
        Ok(hosts)
    }

    /// Stores a short link UID mapping to a target URL.
    ///
    /// `source` names where the link came from — `"navidrome"` for a share the music server itself
    /// minted, anything else (or nothing) for a link that exists only here. Deletion reads it to
    /// decide whether removing the row is the whole job.
    pub fn create_short_link(
        &self,
        id: &str,
        target_url: &str,
        user_id: Option<&str>,
        source: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO short_links (id, target_url, user_id, created_at, source)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, target_url, user_id, unix_now(), source],
        )?;
        Ok(())
    }

    /// Retrieves the target URL for a short link UID.
    ///
    /// Expiry is enforced here. It was not, so a link given a deliberate lifetime kept forwarding
    /// for ever — the column was written and then never read.
    pub fn get_short_link(&self, id: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT target_url FROM short_links
             WHERE id = ?1 AND (expires_at IS NULL OR expires_at > ?2)",
        )?;
        let mut rows = stmt.query(params![id, unix_now()])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    /// Bumps a short link's hit counter. Aggregate only — see migration 6.
    pub fn record_short_link_click(&self, id: &str) {
        let conn = self.conn.lock().unwrap();
        // A counter is never worth failing a redirect over: the visitor gets their page either way.
        let _ = conn.execute(
            "UPDATE short_links SET click_count = click_count + 1, last_clicked_at = ?2
             WHERE id = ?1",
            params![id, unix_now()],
        );
    }

    /// Bumps an ephemeral share's hit counter. Aggregate only — see migration 6.
    pub fn record_share_click(&self, token: &str) {
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute(
            "UPDATE ephemeral_shares SET click_count = click_count + 1, last_clicked_at = ?2
             WHERE token = ?1",
            params![token, unix_now()],
        );
    }

    /// Every link this account has minted, newest first, across both link mechanisms.
    ///
    /// The two tables have almost nothing in common — one holds a hosted audio URL with an RFC3339
    /// expiry, the other a forwarding target with a Unix one — so they are normalised here rather
    /// than in the resolver, which should not have to know that "a link" is two different things.
    pub fn list_links(&self, user_id: &str) -> Result<Vec<LinkRow>> {
        let conn = self.conn.lock().unwrap();
        let mut links = Vec::new();

        let mut stmt = conn.prepare(
            "SELECT id, target_url, created_at, expires_at, click_count, last_clicked_at, source
             FROM short_links WHERE user_id = ?1 ORDER BY created_at DESC",
        )?;
        let mut rows = stmt.query(params![user_id])?;
        while let Some(row) = rows.next()? {
            links.push(LinkRow {
                id: row.get(0)?,
                kind: LinkKind::Short,
                target: row.get(1)?,
                label: None,
                created_at: row.get(2)?,
                expires_at: row.get(3)?,
                click_count: row.get(4)?,
                last_clicked_at: row.get(5)?,
                source: row.get(6)?,
            });
        }

        let mut stmt = conn.prepare(
            "SELECT token, audio_url, track_title, artist_name, expires_at, click_count,
                    last_clicked_at
             FROM ephemeral_shares WHERE user_id = ?1",
        )?;
        let mut rows = stmt.query(params![user_id])?;
        while let Some(row) = rows.next()? {
            let title: String = row.get(2)?;
            let artist: String = row.get(3)?;
            let expires: Option<String> = row.get(4)?;
            links.push(LinkRow {
                id: row.get(0)?,
                kind: LinkKind::Ephemeral,
                target: row.get(1)?,
                label: Some(format!("{artist} — {title}")),
                // Ephemeral shares carry no creation timestamp, only an expiry. Deriving the one
                // from the other would be a guess at the TTL, so the field stays honest and empty.
                created_at: None,
                expires_at: expires.as_deref().and_then(rfc3339_to_unix),
                click_count: row.get(5)?,
                last_clicked_at: row.get(6)?,
                source: None,
            });
        }

        links.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(links)
    }

    /// Deletes a link. Returns the row's `source` so the caller can decide what else has to happen.
    ///
    /// Scoped by `user_id` in the statement itself rather than checked first: a link belonging to
    /// somebody else must not be deletable, and a check-then-delete leaves a window where it is.
    pub fn delete_link(&self, user_id: &str, id: &str, kind: LinkKind) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        match kind {
            LinkKind::Short => {
                let source: Option<String> = conn
                    .query_row(
                        "SELECT source FROM short_links WHERE id = ?1 AND user_id = ?2",
                        params![id, user_id],
                        |row| row.get(0),
                    )
                    .optional()?
                    .flatten();
                let removed = conn.execute(
                    "DELETE FROM short_links WHERE id = ?1 AND user_id = ?2",
                    params![id, user_id],
                )?;
                if removed == 0 {
                    return Err(rusqlite::Error::QueryReturnedNoRows);
                }
                Ok(source)
            }
            LinkKind::Ephemeral => {
                let removed = conn.execute(
                    "DELETE FROM ephemeral_shares WHERE token = ?1 AND user_id = ?2",
                    params![id, user_id],
                )?;
                if removed == 0 {
                    return Err(rusqlite::Error::QueryReturnedNoRows);
                }
                Ok(None)
            }
        }
    }
}

/// One play, as a client reports it.
///
/// `played_at` is RFC3339 and comes from the client, not from this server: a phone that was offline
/// for a day is reporting yesterday's listening, and stamping it on arrival would pile a week of
/// history onto one afternoon.
pub struct ScrobbleEntry {
    pub track_title: String,
    pub artist_name: String,
    pub album_name: Option<String>,
    pub genre: Option<String>,
    pub duration_secs: i64,
    pub played_at: String,
}

/// One play, as stored.
pub struct ScrobbleRow {
    pub track_title: String,
    pub artist_name: String,
    pub album_name: Option<String>,
    pub genre: Option<String>,
    pub duration_secs: i64,
    pub device_name: String,
    pub played_at: String,
}

/// Which of the two link mechanisms a row belongs to. See [`Db::list_links`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LinkKind {
    /// `/listen?id=…` — forwards to a target URL on an allowed host.
    Short,
    /// `/share/<token>` — a hosted audio page with a hard expiry.
    Ephemeral,
}

/// One link, normalised across both tables.
pub struct LinkRow {
    pub id: String,
    pub kind: LinkKind,
    pub target: String,
    /// What the link is *of*, when the row knows. Only ephemeral shares carry track metadata.
    pub label: Option<String>,
    pub created_at: Option<i64>,
    pub expires_at: Option<i64>,
    pub click_count: i64,
    pub last_clicked_at: Option<i64>,
    pub source: Option<String>,
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn rfc3339_to_unix(value: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|dt| dt.timestamp())
}

/// The share-link fields of a settings upsert, grouped so the function keeps a readable signature
/// rather than taking nine positional `Option`s in a row.
#[derive(Default, Clone, Copy)]
pub struct ShareSettingsInput<'a> {
    pub domain: Option<&'a str>,
    pub hosts: Option<&'a str>,
    pub enabled: Option<bool>,
}

/// What an upsert should do with a node's display name.
///
/// A device is named once — when it is paired, or by the user afterwards — and then keeps that
/// name. Everything else that touches the row (a WebSocket connect, a handoff heartbeat) is
/// reporting liveness, not naming anything, and must say so: passing a freshly invented name on
/// every reconnect renamed the device to a new random animal every time the server restarted.
pub enum NodeName<'a> {
    /// Name it. The caller has a name the user chose or supplied.
    Set(&'a str),
    /// Leave the stored name alone; use this one only if the row does not exist yet.
    KeepOr(&'a str),
}

impl NodeName<'_> {
    fn as_str(&self) -> &str {
        match self {
            NodeName::Set(name) | NodeName::KeepOr(name) => name,
        }
    }

    fn overwrites(&self) -> bool {
        matches!(self, NodeName::Set(_))
    }
}

pub struct NodeRecord {
    pub device_id: String,
    pub user_id: String,
    pub petname: String,
    pub client_type: String,
    pub ip_address: Option<String>,
    pub lan_address: Option<String>,
    pub version: Option<String>,
    pub current_track: Option<String>,
    pub last_seen_at: String,
}

pub struct SyncedSettingsRecord {
    pub server_url: Option<String>,
    pub server_username: Option<String>,
    pub lrclib_url: Option<String>,
    pub lyrics_fetch_online: Option<bool>,
    pub stream_format: Option<String>,
    /// The domain the players send share links out on, e.g. `frwd.top`.
    pub share_domain: Option<String>,
    /// Comma-separated hosts `/listen` may forward to.
    pub share_hosts: Option<String>,
    pub share_enabled: Option<bool>,
    pub updated_at: String,
}

pub struct HandoffRecord {
    pub track_uri: String,
    /// How long the track is. 0 when the sender did not say, or when it is a livestream.
    pub duration_ms: i64,
    pub track_title: String,
    pub artist_name: String,
    pub album_name: Option<String>,
    pub artwork_url: Option<String>,
    pub position_ms: i64,
    pub is_playing: bool,
    pub device_id: String,
    pub updated_at: String,
    /// The whole queue as a JSON array, so a resumed session continues rather than stopping after
    /// one track. Kept opaque here: the clients agree on the shape, the server only stores it.
    pub queue_json: Option<String>,
    pub queue_index: Option<i64>,
}

pub struct AppPasswordRecord {
    /// The row's `rowid`, used as the public handle for one credential.
    ///
    /// Labels are chosen by the client and are not unique — several devices calling themselves
    /// `wander-desktop` are the normal case, not an edge one — so a label cannot identify which
    /// credential to revoke. The token itself obviously cannot be the handle. `rowid` is stable,
    /// unique and reveals nothing.
    pub id: i64,
    pub label: String,
    pub created_at: String,
    pub last_used_at: Option<String>,
}

pub struct ShareRecord {
    pub track_title: String,
    pub artist_name: String,
    pub album_name: Option<String>,
    pub audio_url: String,
    pub expires_at: String,
}

#[cfg(all(test, unix))]
mod permission_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// The database and every sidecar SQLite writes beside it. A 0600 database next to a 0644
    /// write-ahead log protects nothing: the `-wal` holds recent writes in plaintext.
    #[test]
    fn the_database_and_its_sidecars_are_owner_only() {
        let dir = std::env::temp_dir().join(format!("agro-perm-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("agro_test.db");

        let db = Db::new(&path).unwrap();
        // Force a write so the -wal exists to be checked.
        drop(db);

        for suffix in ["", "-wal", "-shm"] {
            let mut name = path.as_os_str().to_os_string();
            name.push(suffix);
            let target = std::path::PathBuf::from(name);
            if !target.exists() {
                continue;
            }
            let mode = std::fs::metadata(&target).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "{} is {:o}, not 0600", target.display(), mode);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod node_naming_tests {
    use super::*;

    fn db() -> Db {
        Db::new_in_memory().unwrap()
    }

    fn petname_of(db: &Db, user: &str, device: &str) -> String {
        db.get_active_nodes(user)
            .unwrap()
            .into_iter()
            .find(|n| n.device_id == device)
            .expect("node")
            .petname
    }

    /// The bug this covers: every WebSocket connect invented a name and wrote it, so a device
    /// was renamed to a fresh random animal each time the server restarted.
    #[test]
    fn a_reconnect_does_not_rename_a_named_device() {
        let db = db();
        db.upsert_node("pixel", "alpha", NodeName::Set("Pixel 10"), "wanda", None, None, None, None)
            .unwrap();

        db.upsert_node(
            "pixel",
            "alpha",
            NodeName::KeepOr("Glitchy Alpaca"),
            "wanda",
            None,
            None,
            None,
            None,
        )
        .unwrap();

        assert_eq!(petname_of(&db, "alpha", "pixel"), "Pixel 10");
    }

    #[test]
    fn a_device_seen_for_the_first_time_takes_the_fallback_name() {
        let db = db();
        db.upsert_node(
            "pixel",
            "alpha",
            NodeName::KeepOr("Glitchy Alpaca"),
            "wanda",
            None,
            None,
            None,
            None,
        )
        .unwrap();

        assert_eq!(petname_of(&db, "alpha", "pixel"), "Glitchy Alpaca");
    }

    #[test]
    fn naming_a_device_still_renames_it() {
        let db = db();
        db.upsert_node("pixel", "alpha", NodeName::Set("Pixel 10"), "wanda", None, None, None, None)
            .unwrap();
        db.upsert_node("pixel", "alpha", NodeName::Set("Work phone"), "wanda", None, None, None, None)
            .unwrap();

        assert_eq!(petname_of(&db, "alpha", "pixel"), "Work phone");
    }

    /// Two accounts can choose the same device id, and one must not rename the other's device.
    #[test]
    fn one_account_cannot_rename_another_accounts_device() {
        let db = db();
        db.upsert_node("laptop", "alpha", NodeName::Set("Cachy"), "wander", None, None, None, None)
            .unwrap();
        db.upsert_node("laptop", "delta", NodeName::Set("Lenovo"), "wander", None, None, None, None)
            .unwrap();

        assert_eq!(petname_of(&db, "alpha", "laptop"), "Cachy");
        assert_eq!(petname_of(&db, "delta", "laptop"), "Lenovo");
    }
}

#[cfg(test)]
mod handoff_tests {
    use super::*;

    fn db() -> Db {
        Db::new_in_memory().unwrap()
    }

    fn report_with_duration(
        db: &Db,
        user: &str,
        device: &str,
        title: &str,
        playing: bool,
        duration_ms: i64,
    ) {
        db.update_handoff(
            user,
            &format!("uri:{title}"),
            title,
            "Artist",
            None,
            None,
            0,
            duration_ms,
            playing,
            device,
            None,
            None,
        )
        .unwrap();
    }

    fn report(db: &Db, user: &str, device: &str, title: &str, playing: bool) {
        report_with_duration(db, user, device, title, playing, 0);
    }

    /// The bug this covers: one row per account meant the desktop pausing overwrote the phone's
    /// playing session, and the desktop filters out its own device — so the fleet's track
    /// disappeared at the moment it became the one worth showing.
    #[test]
    fn one_device_pausing_does_not_erase_another_ones_session() {
        let db = db();
        report(&db, "alpha", "phone", "Phone Song", true);
        report(&db, "alpha", "desktop", "Desktop Song", false);

        let elsewhere = db.get_handoff_excluding("alpha", "desktop").unwrap().unwrap();
        assert_eq!(elsewhere.track_title, "Phone Song");
        assert!(elsewhere.is_playing);
        assert_eq!(elsewhere.device_id, "phone");
    }

    /// The original question, unchanged: whatever happened last, whoever it was.
    #[test]
    fn the_accounts_handoff_is_still_the_most_recent_report() {
        let db = db();
        report(&db, "alpha", "phone", "Phone Song", true);
        report(&db, "alpha", "desktop", "Desktop Song", true);

        assert_eq!(db.get_handoff("alpha").unwrap().unwrap().track_title, "Desktop Song");
    }

    #[test]
    fn a_device_updates_its_own_row_rather_than_adding_one() {
        let db = db();
        report(&db, "alpha", "phone", "First", true);
        report(&db, "alpha", "phone", "Second", true);

        assert_eq!(db.get_handoff("alpha").unwrap().unwrap().track_title, "Second");
        assert!(db.get_handoff_excluding("alpha", "phone").unwrap().is_none());
    }

    /// A position with nothing to measure it against can only ever be an elapsed count. The
    /// length travels so whatever renders the session can draw a bar with two ends.
    #[test]
    fn the_track_length_travels_with_the_handoff() {
        let db = db();
        report_with_duration(&db, "alpha", "phone", "Song", true, 214_000);

        assert_eq!(db.get_handoff("alpha").unwrap().unwrap().duration_ms, 214_000);
    }

    /// Zero is "did not say", which is also what a livestream reports. Neither may overwrite a
    /// length another heartbeat already established.
    #[test]
    fn a_heartbeat_without_a_length_keeps_the_one_already_stored() {
        let db = db();
        report_with_duration(&db, "alpha", "phone", "Song", true, 214_000);
        report_with_duration(&db, "alpha", "phone", "Song", true, 0);

        assert_eq!(db.get_handoff("alpha").unwrap().unwrap().duration_ms, 214_000);
    }

    #[test]
    fn one_account_never_sees_anothers_handoff() {
        let db = db();
        report(&db, "alpha", "phone", "Phone Song", true);
        report(&db, "delta", "phone", "Someone Elses Song", true);

        assert_eq!(db.get_handoff("alpha").unwrap().unwrap().track_title, "Phone Song");
        assert!(db.get_handoff_excluding("alpha", "phone").unwrap().is_none());
    }
}
