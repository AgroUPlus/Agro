use rusqlite::{params, Connection, OptionalExtension, Result};
use std::path::Path;
use std::sync::{Arc, Mutex};

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
];

#[derive(Clone)]
pub struct Db {
    /// `pub(crate)` so the library index can keep its own `impl Db` block in `db_library`, rather
    /// than growing this file by another few hundred lines of unrelated SQL.
    pub(crate) conn: Arc<Mutex<Connection>>,
}

impl Db {
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self> {
        let conn = Connection::open(path)?;
        let db = Db {
            conn: Arc::new(Mutex::new(conn)),
        };
        db.init_schema()?;
        db.migrate()?;
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
            "SELECT a.label, a.created_at, a.last_used_at FROM app_passwords a
             JOIN users u ON u.id = a.user_id
             WHERE u.username = ?1 ORDER BY a.created_at DESC",
        )?;
        let rows = stmt.query_map(params![username], |row| {
            Ok(AppPasswordRecord {
                label: row.get(0)?,
                created_at: row.get(1)?,
                last_used_at: row.get(2)?,
            })
        })?;
        rows.collect()
    }

    pub fn revoke_app_password(&self, username: &str, label: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let removed = conn.execute(
            "DELETE FROM app_passwords WHERE label = ?1 AND user_id = (
                 SELECT id FROM users WHERE username = ?2
             )",
            params![label, username],
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
        is_playing: bool,
        device_id: &str,
        queue_json: Option<&str>,
        queue_index: Option<i64>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO handoff_state (user_id, track_uri, track_title, artist_name, album_name, artwork_url, position_ms, is_playing, device_id, updated_at, queue_json, queue_index)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT(user_id) DO UPDATE SET
             track_uri = excluded.track_uri,
             track_title = excluded.track_title,
             artist_name = excluded.artist_name,
             album_name = excluded.album_name,
             artwork_url = excluded.artwork_url,
             position_ms = excluded.position_ms,
             is_playing = excluded.is_playing,
             device_id = excluded.device_id,
             updated_at = excluded.updated_at,
             -- A heartbeat that carries no queue must not erase the one already stored: only a
             -- client that actually sent a queue replaces it.
             queue_json = COALESCE(excluded.queue_json, handoff_state.queue_json),
             queue_index = COALESCE(excluded.queue_index, handoff_state.queue_index)",
            params![user_id, track_uri, track_title, artist_name, album_name, artwork_url, position_ms, is_playing, device_id, now, queue_json, queue_index],
        )?;
        Ok(())
    }

    pub fn get_handoff(&self, user_id: &str) -> Result<Option<HandoffRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT track_uri, track_title, artist_name, album_name, artwork_url, position_ms, is_playing, device_id, updated_at, queue_json, queue_index FROM handoff_state WHERE user_id = ?1")?;
        let mut rows = stmt.query(params![user_id])?;
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
        petname: &str,
        client_type: &str,
        ip_address: Option<&str>,
        version: Option<&str>,
        current_track: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO registered_nodes (device_id, user_id, petname, client_type, ip_address, version, current_track, last_seen_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(device_id) DO UPDATE SET
             user_id = excluded.user_id,
             petname = CASE WHEN excluded.petname != '' THEN excluded.petname ELSE registered_nodes.petname END,
             client_type = excluded.client_type,
             ip_address = COALESCE(excluded.ip_address, registered_nodes.ip_address),
             version = COALESCE(excluded.version, registered_nodes.version),
             current_track = COALESCE(excluded.current_track, registered_nodes.current_track),
             last_seen_at = excluded.last_seen_at",
            params![device_id, user_id, petname, client_type, ip_address, version, current_track, now],
        )?;
        Ok(())
    }

    /// Every registered node, across users. The plugin list needs a whole-deployment view rather
    /// than one user's devices.
    pub fn get_all_nodes(&self) -> Result<Vec<NodeRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT device_id, user_id, petname, client_type, ip_address, version, current_track, last_seen_at
             FROM registered_nodes ORDER BY last_seen_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(NodeRecord {
                device_id: row.get(0)?,
                user_id: row.get(1)?,
                petname: row.get(2)?,
                client_type: row.get(3)?,
                ip_address: row.get(4)?,
                version: row.get(5)?,
                current_track: row.get(6)?,
                last_seen_at: row.get(7)?,
            })
        })?;
        rows.collect()
    }

    pub fn get_active_nodes(&self, user_id: &str) -> Result<Vec<NodeRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT device_id, user_id, petname, client_type, ip_address, version, current_track, last_seen_at
             FROM registered_nodes WHERE user_id = ?1 ORDER BY last_seen_at DESC"
        )?;
        let rows = stmt.query_map(params![user_id], |row| {
            Ok(NodeRecord {
                device_id: row.get(0)?,
                user_id: row.get(1)?,
                petname: row.get(2)?,
                client_type: row.get(3)?,
                ip_address: row.get(4)?,
                version: row.get(5)?,
                current_track: row.get(6)?,
                last_seen_at: row.get(7)?,
            })
        })?;

        let mut nodes = Vec::new();
        for r in rows {
            nodes.push(r?);
        }
        Ok(nodes)
    }

    pub fn upsert_synced_settings(
        &self,
        user_id: &str,
        server_url: Option<&str>,
        server_username: Option<&str>,
        lrclib_url: Option<&str>,
        lyrics_fetch_online: Option<bool>,
        stream_format: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO synced_settings (user_id, server_url, server_username, lrclib_url, lyrics_fetch_online, stream_format, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(user_id) DO UPDATE SET
             server_url = COALESCE(excluded.server_url, synced_settings.server_url),
             server_username = COALESCE(excluded.server_username, synced_settings.server_username),
             lrclib_url = COALESCE(excluded.lrclib_url, synced_settings.lrclib_url),
             lyrics_fetch_online = COALESCE(excluded.lyrics_fetch_online, synced_settings.lyrics_fetch_online),
             stream_format = COALESCE(excluded.stream_format, synced_settings.stream_format),
             updated_at = excluded.updated_at",
            params![user_id, server_url, server_username, lrclib_url, lyrics_fetch_online, stream_format, now],
        )?;
        Ok(())
    }

    pub fn get_synced_settings(&self, user_id: &str) -> Result<Option<SyncedSettingsRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT server_url, server_username, lrclib_url, lyrics_fetch_online, stream_format, updated_at
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
                updated_at: row.get(5)?,
            }))
        } else {
            Ok(None)
        }
    }
}

pub struct NodeRecord {
    pub device_id: String,
    pub user_id: String,
    pub petname: String,
    pub client_type: String,
    pub ip_address: Option<String>,
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
    pub updated_at: String,
}

pub struct HandoffRecord {
    pub track_uri: String,
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
