//! The tables that existed before migrations did.
//!
//! Every database runs this first and then the whole of [`super::migrations`], so nothing may be
//! declared here that a migration also adds.

use rusqlite::Result;

use super::Db;

impl Db {
    pub(super) fn init_schema(&self) -> Result<()> {
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
}
