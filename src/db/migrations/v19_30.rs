//! Migrations 19 to 30. See [`super::MIGRATIONS`].

pub(super) const ENTRIES: &[&str] = &[
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
];
