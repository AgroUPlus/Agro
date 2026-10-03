//! Migrations 10 to 18. See [`super::MIGRATIONS`].

pub(super) const ENTRIES: &[&str] = &[
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
];
