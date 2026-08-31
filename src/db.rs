use chrono::DurationRound;
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
    // 25 — the server stops keeping public IP addresses.
    //
    // Written on every `registerNode`, shown in the dashboard, and consumed by nothing. Neither
    // client has ever sent it: both pass `lanAddress` and leave this null, so the column's only
    // real content came from the dashboard's own view of itself. Against a stolen database it was
    // a location history for no feature's benefit, which makes removing it free rather than a
    // trade.
    //
    // `lan_address` stays. It is an RFC1918 address the LAN peer-to-peer transfer in
    // `db_library::peer_sources_for_track` needs in order to dial a device directly.
    //
    // A plain DROP COLUMN rather than the table rebuild migration 10 had to do: nothing indexes
    // this column, so SQLite can drop it in place.
    "ALTER TABLE registered_nodes DROP COLUMN ip_address;",
    // 26 — a play identifies itself, instead of being identified by its clock reading.
    //
    // Idempotency was `UNIQUE(user_id, artist_name, track_title, played_at)`: a retried outbox did
    // not double-count because the second copy carried the same second-resolution timestamp. That
    // works, but it welds deduplication to the precision of `played_at`, and that precision is a
    // problem of its own — an exact play time reconstructs when someone sleeps, wakes and commutes,
    // which is a thing a stolen database should not contain.
    //
    // With the client naming each play, dedup stops caring what the clock said and the timestamp
    // becomes free to blur. SQLite treats NULLs in a unique index as distinct, so rows from clients
    // that send no id do not collide with each other on the new index.
    //
    // The old rule cannot simply stay alongside it. Once timestamps are rounded to the hour, four
    // plays of one track in one hour share a `played_at`, and an index on
    // `(user_id, artist_name, track_title, played_at)` would reject three of them — the exact
    // data loss the `play_uid` column exists to prevent. So it is rebuilt as a *partial* index that
    // applies only where there is no id: a client on the old protocol keeps the old guarantee and
    // keeps its exact timestamps, and a client on the new one is deduplicated by id and has its
    // clock blurred. One table, two eras, neither breaking the other.
    "
    ALTER TABLE scrobbles ADD COLUMN play_uid TEXT;
    CREATE UNIQUE INDEX IF NOT EXISTS idx_scrobbles_uid ON scrobbles(user_id, play_uid);
    DROP INDEX IF EXISTS idx_scrobbles_unique;
    CREATE UNIQUE INDEX IF NOT EXISTS idx_scrobbles_legacy_unique
        ON scrobbles(user_id, artist_name, track_title, played_at)
        WHERE play_uid IS NULL;
    ",
    // 27 — the server stops being able to read the settings it stores.
    //
    // `synced_settings` held a Navidrome address and username encrypted with `users.settings_key`,
    // a key the server minted and kept in the row next to the ciphertext. Anyone reading the
    // database read both, so the encryption protected nothing that mattering losing the file did
    // not also lose. It was not even working: the write path encrypted with `settings_key` while
    // the read path decrypted with `api_key`, which `create_account` sets to the empty string, and
    // `crypto::decrypt_field` failed *open* — so every account made since migration 9 has been
    // handing clients back raw hex ciphertext where a URL should be.
    //
    // The key moves to the client. It is 32 random bytes the client generates and keeps, and the
    // server holds only `vault_key_wrapped`, that key sealed under one derived from the account
    // passphrase. Deriving it needs the passphrase, and the server keeps nothing but an Argon2
    // hash of that, so the wrapped key is inert here — and typing the passphrase on a new device
    // still recovers everything, which a key that lived only on the devices would not.
    //
    // `has_server_url` is one bit of deliberate plaintext. `sync_mode` has only ever asked whether
    // an address exists, never what it is; with the address inside an opaque blob that question
    // can no longer be answered by looking, so it is answered by an explicit flag instead of by
    // giving the server the means to look.
    //
    // The old columns stay for now. Nothing reads them any more, and dropping them is a separate
    // migration once no client is still writing to them.
    "
    ALTER TABLE users ADD COLUMN vault_salt TEXT;
    ALTER TABLE users ADD COLUMN vault_key_wrapped TEXT;
    ALTER TABLE synced_settings ADD COLUMN settings_blob TEXT;
    ALTER TABLE synced_settings ADD COLUMN has_server_url INTEGER NOT NULL DEFAULT 0;
    ",
    // 28 — the server stops storing local network addresses on disk.
    //
    // `lan_address` was added by migration 22 to let devices discover each other for direct
    // peer-to-peer transfers. A LAN address is volatile: it only exists while a device is on that
    // specific Wi-Fi network, and is only usable while the peer is actively connected via WebSocket.
    //
    // Storing it on disk left stale private IPs in `agro.db` indefinitely. It now lives strictly
    // in RAM on `WsHub` for the duration of the connection.
    "ALTER TABLE registered_nodes DROP COLUMN lan_address;",
    // 29 — device tokens gain an expiry.
    //
    // Until now a token minted once was valid forever. That is what made every other credential
    // control decorative: revoking a passphrase, or enrolling a second factor, left every token
    // ever issued from the old passphrase still working, and there was no way to say "sign me out
    // everywhere" because nothing recorded when a token should stop being one.
    //
    // NULL means "no fixed expiry", which is what every existing row gets and what a deliberately
    // paired device still gets by default — a TUI that logs itself out monthly is worse than no
    // TUI. Idle expiry is computed from `last_used_at` instead and needs no column.
    "ALTER TABLE app_passwords ADD COLUMN expires_at TEXT;",
    // 30 — an append-only record of security-relevant events.
    //
    // Until now nothing recorded that a login had happened, succeeded or failed. `tracing` carried
    // warnings for the operator's own console and nothing else, so the questions a compromised
    // account actually raises — when did this start, which device, from where, what did it do —
    // had no answer anywhere on the server.
    //
    // `user_id` is nullable because a failed login for a username that does not exist is exactly
    // the event most worth recording, and there is no account to point at.
    //
    // The IP is stored truncated (see `audit::truncate_ip`). It is here to make a pattern of
    // attempts visible, which a /24 does as well as a full address, and not to log where anyone
    // lives.
    "CREATE TABLE security_events (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        at TEXT NOT NULL,
        user_id TEXT,
        kind TEXT NOT NULL,
        client_ip TEXT,
        device_label TEXT,
        detail TEXT
    );
    CREATE INDEX security_events_user_at ON security_events(user_id, at DESC);
    CREATE INDEX security_events_at ON security_events(at DESC);
    ",
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
];

/// How long a play keeps its exact timestamp. Past this, no outbox is still holding it, so
/// deduplication no longer needs the seconds and they are rounded away.
const SCROBBLE_EXACT_TIME_DAYS: i64 = 14;

/// How long a device may be quiet before the queue it was playing is scrubbed from its handoff
/// row. Tightened to 2 days to minimize metadata footprint at rest.
const HANDOFF_QUEUE_TTL_DAYS: i64 = 2;

/// How long before the handoff row itself goes. A device silent this long has been replaced or
/// wiped, and its row is a record of what someone was listening to and nothing else.
const HANDOFF_ROW_TTL_DAYS: i64 = 30;

/// How long before an inactive registered node is purged from the database.
const INACTIVE_NODE_TTL_DAYS: i64 = 90;

#[derive(Clone)]
pub struct Db {
    /// `pub(crate)` so the library index can keep its own `impl Db` block in `db_library`, rather
    /// than growing this file by another few hundred lines of unrelated SQL.
    pub(crate) conn: Arc<Mutex<Connection>>,
}

impl Db {
    pub fn get_cached_proxy(&self, url: &str) -> Result<Option<(String, Vec<u8>)>> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().timestamp();
        let mut stmt = conn.prepare("SELECT headers, body FROM proxy_cache WHERE url = ?1 AND expires_at > ?2")?;
        let mut rows = stmt.query(params![url, now])?;
        if let Some(row) = rows.next()? {
            Ok(Some((row.get(0)?, row.get(1)?)))
        } else {
            Ok(None)
        }
    }

    pub fn set_cached_proxy(&self, url: &str, headers: &str, body: &[u8], expires_at: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO proxy_cache (url, headers, body, expires_at) VALUES (?1, ?2, ?3, ?4)",
            params![url, headers, body, expires_at],
        )?;
        Ok(())
    }
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
                -- Dropped again by migration 25, and still declared here: a fresh database runs
                -- `init_schema` and then *every* migration, and migration 10 rebuilds this table
                -- by selecting `ip_address` out of it. Removing it here would abort startup
                -- before the migration that removes it properly ever runs.
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
        // Every table that stores a username or a user id, or this is not a deletion.
        //
        // It used to be five of them. What survived was the whole social graph — friendships in
        // both directions, drops sent and received, jam membership and votes — plus every scrobble,
        // which is a listening history, and every live share link, which kept working after the
        // account that minted it was gone. "Deleted" has to mean deleted.
        //
        // Two columns are named differently everywhere, so this is a list rather than a loop: some
        // tables key on `users.id` and most key on the username, and `track_drops`/`friendships`
        // key on *two* user columns each.
        let by_id: &[&str] = &["app_passwords", "totp_recovery_codes", "federated_identities"];
        for table in by_id {
            conn.execute(
                &format!("DELETE FROM {table} WHERE user_id = ?1"),
                params![user_id],
            )?;
        }

        let by_username: &[&str] = &[
            "registered_nodes",
            "device_holdings",
            "handoff_state",
            "synced_settings",
            "scrobbles",
            "friend_codes",
            "ephemeral_shares",
            "short_links",
            "spool_items",
            "upload_sessions",
        ];
        for table in by_username {
            conn.execute(
                &format!("DELETE FROM {table} WHERE user_id = ?1"),
                params![username],
            )?;
        }

        // Friendship is two rows, one per direction. Removing only the row this account owns leaves
        // the other person still holding a friendship with somebody who no longer exists.
        conn.execute(
            "DELETE FROM friendships WHERE user_id = ?1 OR friend_id = ?1",
            params![username],
        )?;
        // A drop is addressed: sent and received both have to go.
        conn.execute(
            "DELETE FROM track_drops WHERE from_user = ?1 OR to_user = ?1",
            params![username],
        )?;
        conn.execute(
            "DELETE FROM listen_along WHERE listener_id = ?1 OR host_id = ?1",
            params![username],
        )?;
        conn.execute("DELETE FROM jam_members WHERE username = ?1", params![username])?;
        conn.execute("DELETE FROM jam_votes WHERE username = ?1", params![username])?;
        conn.execute("DELETE FROM jam_skips WHERE username = ?1", params![username])?;
        conn.execute("DELETE FROM jam_tracks WHERE added_by = ?1", params![username])?;
        conn.execute("DELETE FROM jams WHERE host = ?1", params![username])?;

        // The audit trail is the one thing kept, and only in a form that names nobody: "an account
        // was deleted" is a fact the operator needs, and rows still carrying the username would be
        // a record of the person who asked to be forgotten.
        conn.execute(
            "UPDATE security_events SET user_id = NULL, client_ip = NULL, device_label = NULL
              WHERE user_id = ?1",
            params![username],
        )?;

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
        content_hash: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO handoff_state (user_id, track_uri, track_title, artist_name, album_name, artwork_url, position_ms, is_playing, device_id, updated_at, queue_json, queue_index, duration_ms, content_hash)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
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
             queue_index = COALESCE(excluded.queue_index, handoff_state.queue_index),
             -- Same rule as the queue: a heartbeat that does not name a hash must not erase the
             -- one the track change already established.
             content_hash = COALESCE(excluded.content_hash, handoff_state.content_hash)",
            params![user_id, track_uri, track_title, artist_name, album_name, artwork_url, position_ms, is_playing, device_id, now, queue_json, queue_index, duration_ms, content_hash],
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
                    is_playing, device_id, updated_at, queue_json, queue_index, duration_ms,
                    content_hash
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
                content_hash: row.get(12)?,
            }))
        } else {
            Ok(None)
        }
    }

    /// Rounds a play time down to the hour it fell in.
    ///
    /// An exact play time is a lifestyle record: a run of them says when someone woke, commuted,
    /// worked and went to bed, and that is the single most identifying thing in this database.
    /// Every statistic Agro computes — the 24-bar hour histogram, the day sparkline, the 8-week
    /// heatmap, streaks, top artists, taste match — buckets by hour or by day already, so the
    /// seconds are precision nobody reads.
    ///
    /// An unparseable timestamp is returned untouched. It is already excluded from every timeline
    /// (`stats::compute` counts it in totals but cannot place it), and inventing an hour for it
    /// would be worse than leaving it alone.
    fn to_hour(played_at: &str) -> String {
        match chrono::DateTime::parse_from_rfc3339(played_at) {
            Ok(dt) => dt
                .with_timezone(&chrono::Utc)
                .duration_trunc(chrono::Duration::hours(1))
                .map(|t| t.to_rfc3339())
                .unwrap_or_else(|_| played_at.to_string()),
            Err(_) => played_at.to_string(),
        }
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
                      device_name, played_at, client_type, play_uid)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            )?;
            for entry in entries {
                // Only a play that names itself may be blurred. Without a `play_uid` the unique
                // index on `(user_id, artist_name, track_title, played_at)` is still the only thing
                // standing between a retried outbox and double-counted plays, and rounding the
                // timestamp would collapse four plays of one track in one hour into one row —
                // silently breaking the on-repeat feed and deflating every count that feeds it.
                // A client that sends an id has moved its idempotency off the clock, so the clock
                // is free to lose its seconds.
                let played_at = match entry.play_uid {
                    Some(_) => Self::to_hour(&entry.played_at),
                    None => entry.played_at.clone(),
                };
                inserted += stmt.execute(params![
                    user_id,
                    entry.track_title,
                    entry.artist_name,
                    entry.album_name,
                    entry.genre,
                    entry.duration_secs,
                    device_name,
                    played_at,
                    client_type,
                    entry.play_uid,
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

    /// Purges scrobbles for an account, optionally restricted by year or before a given timestamp.
    ///
    /// Allows users to actively wipe listening history (e.g. at the conclusion of viewing a Rewind).
    pub fn purge_scrobbles(
        &self,
        user_id: &str,
        year: Option<i32>,
        before: Option<&str>,
    ) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let count = match (year, before) {
            (Some(y), _) => {
                let start = format!("{y:04}-01-01T00:00:00+00:00");
                let end = format!("{:04}-01-01T00:00:00+00:00", y + 1);
                conn.execute(
                    "DELETE FROM scrobbles WHERE user_id = ?1 AND played_at >= ?2 AND played_at < ?3",
                    params![user_id, start, end],
                )?
            }
            (None, Some(b)) => conn.execute(
                "DELETE FROM scrobbles WHERE user_id = ?1 AND played_at < ?2",
                params![user_id, b],
            )?,
            (None, None) => conn.execute(
                "DELETE FROM scrobbles WHERE user_id = ?1",
                params![user_id],
            )?,
        };
        Ok(count)
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

    /// Deletes what has expired, and strips the residue from what has gone quiet.
    ///
    /// Nothing in this database was ever swept. `ephemeral_shares`, `friend_codes` and
    /// `short_links` all carry an expiry that was only ever consulted in a `WHERE` clause at read
    /// time, so an expired row stopped being *usable* but never stopped being *readable* — it sat
    /// in the file, and in every backup of it, indefinitely. `friend_codes` even has an index on
    /// `expires_at` that until now nothing used.
    ///
    /// `handoff_state` is the different case. It is keyed `(user_id, device_id)` and overwritten
    /// constantly, so it does not grow; what accumulates is the `queue_json` blob on the row of a
    /// device someone stopped using, which is a snapshot of what they were listening to, kept
    /// forever. Scrubbing the queue is the part that matters, and it is done separately from
    /// deleting the row: an active device's row would only be recreated by its next `updateHandoff`
    /// anyway, so there is no point racing it.
    ///
    /// Returns nothing and takes no arguments because there is nothing a caller could usefully do
    /// with the result. Failures are logged, not propagated: a sweep that cannot run is not a
    /// reason to take the ticker down with it.
    pub fn sweep_retention(&self) {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now();
        let now_rfc = now.to_rfc3339();
        let now_unix = now.timestamp();

        let run = |what: &str, sql: &str, args: &[&dyn rusqlite::ToSql]| {
            match conn.execute(sql, args) {
                Ok(n) if n > 0 => tracing::debug!("retention sweep: {what} removed {n} rows"),
                Ok(_) => {}
                Err(e) => tracing::warn!("retention sweep: {what} failed: {e}"),
            }
        };

        run(
            "ephemeral_shares",
            "DELETE FROM ephemeral_shares WHERE expires_at < ?1",
            params![now_rfc],
        );
        run(
            "friend_codes",
            "DELETE FROM friend_codes WHERE expires_at < ?1",
            params![now_rfc],
        );
        // A null `expires_at` is a link that was minted without one, which means it does not
        // expire. Only rows that named a deadline and are past it go.
        run(
            "short_links",
            "DELETE FROM short_links WHERE expires_at IS NOT NULL AND expires_at < ?1",
            params![now_unix],
        );

        let stale = (now - chrono::Duration::days(HANDOFF_QUEUE_TTL_DAYS)).to_rfc3339();
        run(
            "handoff queues",
            "UPDATE handoff_state
                SET queue_json = NULL, queue_index = NULL, position_ms = 0
              WHERE updated_at < ?1 AND queue_json IS NOT NULL",
            params![stale],
        );

        let dead = (now - chrono::Duration::days(HANDOFF_ROW_TTL_DAYS)).to_rfc3339();
        run(
            "handoff rows",
            "DELETE FROM handoff_state WHERE updated_at < ?1",
            params![dead],
        );

        // Opt-in, and off by default. A listening history is the point of the product for some
        // people and a liability for others, so the operator chooses — but an upgrade must never
        // silently delete years of it, which is what a default would do.
        if let Some(days) = std::env::var("AGRO_SCROBBLE_RETENTION_DAYS")
            .ok()
            .and_then(|v| v.trim().parse::<i64>().ok())
            .filter(|d| *d > 0)
        {
            let cutoff = (now - chrono::Duration::days(days)).to_rfc3339();
            run(
                "scrobbles",
                "DELETE FROM scrobbles WHERE played_at < ?1",
                params![cutoff],
            );
        }

        let stale_nodes = (now - chrono::Duration::days(INACTIVE_NODE_TTL_DAYS)).to_rfc3339();
        run(
            "inactive nodes",
            "DELETE FROM registered_nodes WHERE last_seen_at < ?1",
            params![stale_nodes],
        );

        self.coarsen_settled_scrobbles(&conn, now);
    }

    /// Rounds down the play times of history old enough that nothing will be re-sent for it.
    ///
    /// Ingest blurs a timestamp only when the client named the play (see `record_scrobbles`), which
    /// leaves two kinds of exact row behind: everything recorded before any of this existed, and
    /// anything still arriving from a client that has not been updated. Both stop being at risk of
    /// a retry once they are old enough — an outbox does not hold a fortnight — so past that point
    /// the seconds can go the same way.
    ///
    /// Done in SQL rather than by reading rows into Rust because the timestamps are RFC3339 in a
    /// fixed-width UTC form and truncation is a string operation on them. Rows whose format does
    /// not match are left alone, exactly as `to_hour` leaves an unparseable value alone.
    fn coarsen_settled_scrobbles(&self, conn: &Connection, now: chrono::DateTime<chrono::Utc>) {
        let cutoff = (now - chrono::Duration::days(SCROBBLE_EXACT_TIME_DAYS)).to_rfc3339();
        // `OR IGNORE` so one collision does not abort the batch: two exact plays of a track in the
        // same hour cannot both round to it under the legacy partial index, and the right outcome
        // is to leave that pair alone and coarsen everything else, not to give up on all of it.
        let sql = "UPDATE OR IGNORE scrobbles
                      SET played_at = substr(played_at, 1, 13) || ':00:00+00:00'
                    WHERE played_at < ?1
                      AND substr(played_at, 14) != ':00:00+00:00'
                      AND length(played_at) >= 19";
        match conn.execute(sql, params![cutoff]) {
            Ok(n) if n > 0 => tracing::debug!("retention sweep: coarsened {n} play times"),
            Ok(_) => {}
            Err(e) => tracing::warn!("retention sweep: coarsening play times failed: {e}"),
        }
    }

    pub fn upsert_node(
        &self,
        device_id: &str,
        user_id: &str,
        petname: NodeName<'_>,
        client_type: &str,
        version: Option<&str>,
        current_track: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        // The name is decided in SQL rather than by reading the row first, so a heartbeat that
        // arrives while the user is renaming the device cannot write back the name it read.
        conn.execute(
            "INSERT INTO registered_nodes (device_id, user_id, petname, client_type, version, current_track, last_seen_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(user_id, device_id) DO UPDATE SET
             petname = CASE WHEN ?8 AND excluded.petname != '' THEN excluded.petname ELSE registered_nodes.petname END,
             client_type = excluded.client_type,
             version = COALESCE(excluded.version, registered_nodes.version),
             current_track = COALESCE(excluded.current_track, registered_nodes.current_track),
             last_seen_at = excluded.last_seen_at",
            params![
                device_id,
                user_id,
                petname.as_str(),
                client_type,
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
            "SELECT device_id, user_id, petname, client_type, version, current_track, last_seen_at
             FROM registered_nodes ORDER BY last_seen_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(NodeRecord {
                device_id: row.get(0)?,
                user_id: row.get(1)?,
                petname: row.get(2)?,
                client_type: row.get(3)?,
                version: row.get(4)?,
                current_track: row.get(5)?,
                last_seen_at: row.get(6)?,
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
            "SELECT device_id, user_id, petname, client_type, version, current_track, last_seen_at
             FROM registered_nodes WHERE user_id = ?1 ORDER BY last_seen_at DESC"
        )?;
        let rows = stmt.query_map(params![user_id], |row| {
            Ok(NodeRecord {
                device_id: row.get(0)?,
                user_id: row.get(1)?,
                petname: row.get(2)?,
                client_type: row.get(3)?,
                version: row.get(4)?,
                current_track: row.get(5)?,
                last_seen_at: row.get(6)?,
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
        // The holdings go with it. They are a claim about what is on a machine, and the machine is
        // gone; left behind they are an unbounded pile of rows describing files nothing can serve.
        // The readers ignore unregistered holders now, so this is hygiene rather than the fix, but
        // a device retired and re-paired would otherwise carry stale claims back with it.
        conn.execute(
            "DELETE FROM device_holdings WHERE user_id = ?1 AND device_id = ?2",
            params![user_id, device_id],
        )?;
        Ok(affected > 0)
    }

    pub fn upsert_synced_settings(
        &self,
        user_id: &str,
        settings_blob: Option<&str>,
        has_server_url: Option<bool>,
        lyrics_fetch_online: Option<bool>,
        stream_format: Option<&str>,
        share: ShareSettingsInput<'_>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO synced_settings (user_id, settings_blob, has_server_url, lyrics_fetch_online, stream_format, share_domain, share_hosts, share_enabled, updated_at)
             VALUES (?1, ?2, COALESCE(?3, 0), ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(user_id) DO UPDATE SET
             settings_blob = COALESCE(excluded.settings_blob, synced_settings.settings_blob),
             has_server_url = COALESCE(?3, synced_settings.has_server_url),
             lyrics_fetch_online = COALESCE(excluded.lyrics_fetch_online, synced_settings.lyrics_fetch_online),
             stream_format = COALESCE(excluded.stream_format, synced_settings.stream_format),
             share_domain = COALESCE(excluded.share_domain, synced_settings.share_domain),
             share_hosts = COALESCE(excluded.share_hosts, synced_settings.share_hosts),
             share_enabled = COALESCE(excluded.share_enabled, synced_settings.share_enabled),
             updated_at = excluded.updated_at",
            params![
                user_id,
                settings_blob,
                has_server_url,
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
            "SELECT settings_blob, has_server_url, lyrics_fetch_online, stream_format,
                    share_domain, share_hosts, share_enabled, updated_at
             FROM synced_settings WHERE user_id = ?1"
        )?;
        let mut rows = stmt.query(params![user_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(SyncedSettingsRecord {
                settings_blob: row.get(0)?,
                has_server_url: row.get::<_, i64>(1)? != 0,
                lyrics_fetch_online: row.get(2)?,
                stream_format: row.get(3)?,
                share_domain: row.get(4)?,
                share_hosts: row.get(5)?,
                share_enabled: row.get(6)?,
                updated_at: row.get(7)?,
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
        expires_at: Option<i64>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO short_links (id, target_url, user_id, created_at, source, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, target_url, user_id, unix_now(), source, expires_at],
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
    /// What makes a retry a retry. Minted by the client when the play happens and kept in its
    /// outbox, so the same play re-sent carries the same id however many times it is offered.
    /// `None` from a client that predates this, which falls back to the timestamp rule.
    pub play_uid: Option<String>,
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
    pub version: Option<String>,
    pub current_track: Option<String>,
    pub last_seen_at: String,
}

pub struct SyncedSettingsRecord {
    /// The account's upstream settings, sealed by the client under a key this server does not
    /// have. Opaque here by design: it is stored, returned, and never inspected.
    pub settings_blob: Option<String>,
    /// Whether [`Self::settings_blob`] contains a server address. The one thing the server needs
    /// to know about the contents, stated outright rather than discovered by decrypting.
    pub has_server_url: bool,
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
    /// SHA-256 of the bytes being played, when the sender knows it. `None` for anything that is
    /// not a hashed local file, which is what makes a direct transfer impossible and a name match
    /// the only option left.
    pub content_hash: Option<String>,
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
mod settings_vault_tests {
    use super::*;

    fn db() -> Db {
        Db::new_in_memory().unwrap()
    }

    fn share() -> ShareSettingsInput<'static> {
        ShareSettingsInput { domain: None, hosts: None, enabled: None }
    }

    /// The blob goes in and comes out unchanged, and nothing on the way tries to interpret it.
    #[test]
    fn a_settings_blob_round_trips_untouched() {
        let db = db();
        let sealed = "6e6f742d612d75726c-deadbeef";
        db.upsert_synced_settings("alpha", Some(sealed), Some(true), Some(true), Some("FLAC"), share())
            .unwrap();

        let got = db.get_synced_settings("alpha").unwrap().unwrap();
        assert_eq!(got.settings_blob.as_deref(), Some(sealed));
        assert!(got.has_server_url);
    }

    /// What a stolen database would actually show. The sealed bytes are the only trace of the
    /// address, and the old plaintext columns are never written to again.
    #[test]
    fn no_readable_address_reaches_the_table() {
        let db = db();
        db.upsert_synced_settings(
            "alpha",
            Some("0ff1ce-sealed-bytes"),
            Some(true),
            None,
            None,
            share(),
        )
        .unwrap();

        let conn = db.conn.lock().unwrap();
        let (blob, url, user): (String, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT settings_blob, server_url, server_username FROM synced_settings",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();

        assert_eq!(blob, "0ff1ce-sealed-bytes");
        assert_eq!(url, None, "the legacy plaintext column must stay empty");
        assert_eq!(user, None);
    }

    /// A partial update — toggling a preference — must not blank the sealed settings.
    #[test]
    fn updating_a_preference_leaves_the_blob_alone() {
        let db = db();
        db.upsert_synced_settings("alpha", Some("sealed"), Some(true), Some(true), None, share())
            .unwrap();
        db.upsert_synced_settings("alpha", None, None, Some(false), None, share())
            .unwrap();

        let got = db.get_synced_settings("alpha").unwrap().unwrap();
        assert_eq!(got.settings_blob.as_deref(), Some("sealed"));
        assert_eq!(got.lyrics_fetch_online, Some(false));
        assert!(got.has_server_url, "and the flag is not reset either");
    }

    /// Clearing the address has to be expressible, or `syncMode` would be stuck reporting
    /// Navidrome for ever once it had been set.
    #[test]
    fn the_server_url_flag_can_be_turned_off() {
        let db = db();
        db.upsert_synced_settings("alpha", Some("sealed"), Some(true), None, None, share())
            .unwrap();
        db.upsert_synced_settings("alpha", Some("resealed"), Some(false), None, None, share())
            .unwrap();

        assert!(!db.get_synced_settings("alpha").unwrap().unwrap().has_server_url);
    }
}

#[cfg(test)]
mod scrobble_time_tests {
    use super::*;

    fn db() -> Db {
        Db::new_in_memory().unwrap()
    }

    fn entry(title: &str, at: &str, uid: Option<&str>) -> ScrobbleEntry {
        ScrobbleEntry {
            track_title: title.into(),
            artist_name: "Boards of Canada".into(),
            album_name: None,
            genre: None,
            duration_secs: 200,
            played_at: at.into(),
            play_uid: uid.map(str::to_string),
        }
    }

    fn times(db: &Db) -> Vec<String> {
        let conn = db.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT played_at FROM scrobbles ORDER BY id")
            .unwrap();
        let rows = stmt.query_map([], |r| r.get(0)).unwrap();
        rows.map(|r| r.unwrap()).collect()
    }

    /// The whole reason `play_uid` exists. Four plays of one track inside one hour must stay four
    /// rows after their timestamps are rounded into the same bucket — the on-repeat feed needs
    /// four, and the old timestamp-keyed index would have left one.
    #[test]
    fn repeat_plays_in_one_hour_survive_coarsening() {
        let db = db();
        let batch: Vec<_> = ["03:05:11", "03:19:40", "03:31:02", "03:58:59"]
            .iter()
            .enumerate()
            .map(|(i, t)| {
                entry(
                    "Roygbiv",
                    &format!("2026-08-29T{t}+00:00"),
                    Some(&format!("uid-{i}")),
                )
            })
            .collect();

        assert_eq!(db.record_scrobbles("alpha", "phone", None, &batch).unwrap(), 4);

        let stored = times(&db);
        assert_eq!(stored.len(), 4, "one row per play");
        assert!(
            stored.iter().all(|t| t == &stored[0]),
            "all four land in the same hour bucket: {stored:?}"
        );
        assert!(stored[0].contains("T03:00:00"), "rounded down: {stored:?}");
    }

    /// The retry case the unique index was built for, now keyed on the id instead of the clock.
    #[test]
    fn a_resent_batch_inserts_nothing() {
        let db = db();
        let batch = vec![entry("Dayvan Cowboy", "2026-08-29T03:05:11+00:00", Some("uid-a"))];

        assert_eq!(db.record_scrobbles("alpha", "phone", None, &batch).unwrap(), 1);
        assert_eq!(
            db.record_scrobbles("alpha", "phone", None, &batch).unwrap(),
            0,
            "the same play offered twice is still one play"
        );
        assert_eq!(times(&db).len(), 1);
    }

    /// Two accounts may both play a track at the same moment; the id is only unique within one.
    #[test]
    fn the_same_uid_under_two_accounts_is_two_plays() {
        let db = db();
        let batch = vec![entry("Olson", "2026-08-29T03:05:11+00:00", Some("uid-a"))];
        db.record_scrobbles("alpha", "phone", None, &batch).unwrap();
        assert_eq!(db.record_scrobbles("delta", "phone", None, &batch).unwrap(), 1);
    }

    /// A client that has not been updated keeps the exact timestamp, because the timestamp is
    /// still the only thing making its retries idempotent.
    #[test]
    fn a_client_without_a_uid_keeps_its_exact_timestamp() {
        let db = db();
        let batch = vec![entry("Amo Bishop Roden", "2026-08-29T03:05:11+00:00", None)];

        db.record_scrobbles("alpha", "phone", None, &batch).unwrap();
        assert_eq!(times(&db), vec!["2026-08-29T03:05:11+00:00"]);

        // And it is still deduplicated the old way.
        assert_eq!(db.record_scrobbles("alpha", "phone", None, &batch).unwrap(), 0);
    }

    /// Settled history is rounded by the sweep even when it arrived without an id, which is how
    /// rows written before any of this get cleaned up.
    #[test]
    fn the_sweep_coarsens_old_exact_timestamps() {
        let db = db();
        let old = (chrono::Utc::now() - chrono::Duration::days(SCROBBLE_EXACT_TIME_DAYS + 1))
            .with_timezone(&chrono::Utc)
            .format("%Y-%m-%dT%H:%M:%S+00:00")
            .to_string();
        let recent = (chrono::Utc::now() - chrono::Duration::days(1))
            .format("%Y-%m-%dT%H:%M:%S+00:00")
            .to_string();

        db.record_scrobbles(
            "alpha",
            "phone",
            None,
            &[entry("Old", &old, None), entry("Recent", &recent, None)],
        )
        .unwrap();

        db.sweep_retention();

        let stored = times(&db);
        assert!(
            stored[0].ends_with(":00:00+00:00"),
            "settled history should be rounded: {stored:?}"
        );
        assert_eq!(
            stored[1], recent,
            "recent history may still be retried, so it keeps its seconds"
        );
    }

    /// Rounding must not invent a time for something it cannot read.
    #[test]
    fn an_unparseable_timestamp_is_left_alone() {
        let db = db();
        db.record_scrobbles(
            "alpha",
            "phone",
            None,
            &[entry("Broken", "not a date", Some("uid-x"))],
        )
        .unwrap();
        assert_eq!(times(&db), vec!["not a date"]);
    }
}

#[cfg(test)]
mod retention_tests {
    use super::*;

    fn db() -> Db {
        Db::new_in_memory().unwrap()
    }

    fn ago(days: i64) -> String {
        (chrono::Utc::now() - chrono::Duration::days(days)).to_rfc3339()
    }

    fn count(db: &Db, table: &str) -> i64 {
        db.conn
            .lock()
            .unwrap()
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }

    /// The point of the sweep: expiry stopped a row being *usable* long ago, but only this makes
    /// it stop being *readable* by anyone who takes the file.
    #[test]
    fn expired_rows_are_deleted_and_live_ones_are_not() {
        let db = db();
        {
            let conn = db.conn.lock().unwrap();
            for (token, expires) in [("dead", ago(1)), ("live", ago(-1))] {
                conn.execute(
                    "INSERT INTO ephemeral_shares
                        (token, user_id, track_title, artist_name, album_name, audio_url, expires_at)
                     VALUES (?1, 'alpha', 't', 'a', NULL, 'http://x', ?2)",
                    params![token, expires],
                )
                .unwrap();
            }
            for (code, expires) in [("DEAD", ago(1)), ("LIVE", ago(-1))] {
                conn.execute(
                    "INSERT INTO friend_codes (code, user_id, created_at, expires_at)
                     VALUES (?1, 'alpha', ?2, ?3)",
                    params![code, ago(2), expires],
                )
                .unwrap();
            }
            let now = chrono::Utc::now().timestamp();
            for (id, expires) in [
                ("dead", Some(now - 60)),
                ("live", Some(now + 3600)),
                // No deadline named, so it does not expire and must survive.
                ("forever", None),
            ] {
                conn.execute(
                    "INSERT INTO short_links (id, target_url, user_id, created_at, expires_at)
                     VALUES (?1, 'http://x', 'alpha', ?2, ?3)",
                    params![id, now, expires],
                )
                .unwrap();
            }
        }

        db.sweep_retention();

        assert_eq!(count(&db, "ephemeral_shares"), 1);
        assert_eq!(count(&db, "friend_codes"), 1);
        assert_eq!(count(&db, "short_links"), 2);
    }

    /// A device that went quiet keeps its row — the account still wants to know it exists — but
    /// the queue it was playing is what a stolen database would read, and that goes.
    #[test]
    fn a_stale_handoff_keeps_its_row_but_loses_its_queue() {
        let db = db();
        db.conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO handoff_state
                    (user_id, device_id, track_uri, track_title, artist_name, position_ms,
                     is_playing, updated_at, queue_json, queue_index)
                 VALUES ('alpha', 'laptop', 'u', 't', 'a', 91000, 0, ?1, '[\"one\",\"two\"]', 1)",
                params![ago(10)],
            )
            .unwrap();

        db.sweep_retention();

        let (queue, index, position): (Option<String>, Option<i64>, i64) = db
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT queue_json, queue_index, position_ms FROM handoff_state",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();

        assert_eq!(queue, None, "the stale queue should have been scrubbed");
        assert_eq!(index, None);
        assert_eq!(position, 0);
        assert_eq!(count(&db, "handoff_state"), 1, "the row itself stays");
    }

    #[test]
    fn a_long_dead_handoff_row_is_removed_entirely() {
        let db = db();
        db.conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO handoff_state
                    (user_id, device_id, track_uri, track_title, artist_name, position_ms,
                     is_playing, updated_at)
                 VALUES ('alpha', 'retired', 'u', 't', 'a', 0, 0, ?1)",
                params![ago(HANDOFF_ROW_TTL_DAYS + 1)],
            )
            .unwrap();

        db.sweep_retention();

        assert_eq!(count(&db, "handoff_state"), 0);
    }

    /// A device in daily use must not have the queue pulled out from under it.
    #[test]
    fn an_active_handoff_is_left_alone() {
        let db = db();
        db.update_handoff(
            "alpha",
            "u",
            "t",
            "a",
            None,
            None,
            42_000,
            180_000,
            true,
            "phone",
            Some("[\"one\"]"),
            Some(0),
            None,
        )
        .unwrap();

        db.sweep_retention();

        let handoff = db.get_handoff("alpha").unwrap().expect("row survives");
        assert_eq!(handoff.position_ms, 42_000);
        assert!(handoff.queue_json.is_some(), "an active queue must survive");
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
        db.upsert_node("pixel", "alpha", NodeName::Set("Pixel 10"), "wanda", None, None)
            .unwrap();

        db.upsert_node(
            "pixel",
            "alpha",
            NodeName::KeepOr("Glitchy Alpaca"),
            "wanda",
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
        )
        .unwrap();

        assert_eq!(petname_of(&db, "alpha", "pixel"), "Glitchy Alpaca");
    }

    #[test]
    fn naming_a_device_still_renames_it() {
        let db = db();
        db.upsert_node("pixel", "alpha", NodeName::Set("Pixel 10"), "wanda", None, None)
            .unwrap();
        db.upsert_node("pixel", "alpha", NodeName::Set("Work phone"), "wanda", None, None)
            .unwrap();

        assert_eq!(petname_of(&db, "alpha", "pixel"), "Work phone");
    }

    /// Two accounts can choose the same device id, and one must not rename the other's device.
    #[test]
    fn one_account_cannot_rename_another_accounts_device() {
        let db = db();
        db.upsert_node("laptop", "alpha", NodeName::Set("Cachy"), "wander", None, None)
            .unwrap();
        db.upsert_node("laptop", "delta", NodeName::Set("Lenovo"), "wander", None, None)
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

    #[test]
    fn public_key_can_be_set_and_read_on_profile() {
        let db = db();
        db.create_account("alpha", "pass", crate::db_identity::Role::Admin, crate::db_identity::AccountState::Active).unwrap();
        assert_eq!(db.profile("alpha").unwrap().unwrap().public_key, None);
        assert!(db.set_public_key("alpha", Some("base64-pubkey-xyz")).unwrap());
        assert_eq!(
            db.profile("alpha").unwrap().unwrap().public_key.as_deref(),
            Some("base64-pubkey-xyz")
        );
        assert!(db.set_public_key("alpha", None).unwrap());
        assert_eq!(db.profile("alpha").unwrap().unwrap().public_key, None);
    }

    #[test]
    fn e2ee_encrypted_drop_stores_and_reads_ciphertext() {
        let db = db();
        let new_drop = crate::db_drops::NewDrop {
            track_title: "Secret Track".to_string(),
            artist_name: "Secret Artist".to_string(),
            note: None,
            note_ciphertext: Some("sealed-ciphertext-payload-base64".to_string()),
            is_encrypted: true,
            ..Default::default()
        };
        let drop_id = db.create_drop("alpha", "beta", &new_drop).unwrap();
        let inbox = db.inbox("beta", 10, 0).unwrap();
        let found = inbox.iter().find(|d| d.id == drop_id).unwrap();
        assert_eq!(found.note_ciphertext.as_deref(), Some("sealed-ciphertext-payload-base64"));
        assert!(found.is_encrypted);
        assert_eq!(found.note, None);
    }

    #[test]
    fn purge_scrobbles_by_year_and_all() {
        let db = db();
        let batch = vec![
            ScrobbleEntry {
                track_title: "Track 2024".to_string(),
                artist_name: "Artist".to_string(),
                album_name: None,
                genre: None,
                duration_secs: 180,
                played_at: "2024-06-15T12:00:00+00:00".to_string(),
                play_uid: Some("u1".to_string()),
            },
            ScrobbleEntry {
                track_title: "Track 2025".to_string(),
                artist_name: "Artist".to_string(),
                album_name: None,
                genre: None,
                duration_secs: 200,
                played_at: "2025-03-10T14:00:00+00:00".to_string(),
                play_uid: Some("u2".to_string()),
            },
            ScrobbleEntry {
                track_title: "Track 2025 B".to_string(),
                artist_name: "Artist".to_string(),
                album_name: None,
                genre: None,
                duration_secs: 210,
                played_at: "2025-08-20T16:00:00+00:00".to_string(),
                play_uid: Some("u3".to_string()),
            },
        ];
        db.record_scrobbles("alpha", "phone", None, &batch).unwrap();
        assert_eq!(db.scrobble_rows("alpha", None, None).unwrap().len(), 3);

        // Purge 2024
        let purged_2024 = db.purge_scrobbles("alpha", Some(2024), None).unwrap();
        assert_eq!(purged_2024, 1);
        assert_eq!(db.scrobble_rows("alpha", None, None).unwrap().len(), 2);

        // Purge all remaining
        let purged_all = db.purge_scrobbles("alpha", None, None).unwrap();
        assert_eq!(purged_all, 2);
        assert_eq!(db.scrobble_rows("alpha", None, None).unwrap().len(), 0);
    }
}
