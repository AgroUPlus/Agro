//! The database: one SQLite connection behind a mutex, and the methods on it.
//!
//! The methods are spread across `db/` by subject, and further across the `db_*` files beside this
//! one, each with its own `impl Db` block. This file holds only the handle and how it is opened.

use rusqlite::{Connection, Result};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

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
        if let Err(error) =
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600))
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

mod erasure;
mod handoff;
mod init_schema;
mod links;
mod migrations;
mod nodes;
mod plugins;
mod pool;
mod proxy_cache;
mod retention;
mod scrobbles;
mod settings;
mod shares;
#[cfg(test)]
mod tests;
mod tokens;
mod users;

pub use links::LinkKind;
pub use nodes::{declared_client_type, inferred_client_type, NodeName, NodeRecord};
pub use scrobbles::{ScrobbleEntry, ScrobbleRow};
pub use settings::ShareSettingsInput;

#[derive(Clone)]
pub struct Db {
    /// `pub(crate)` so the library index can keep its own `impl Db` block in `db_library`, rather
    /// than growing this file by another few hundred lines of unrelated SQL.
    pub(crate) conn: Arc<Mutex<Connection>>,
    /// Read-only connections for methods that only SELECT. See [`pool`].
    readers: Arc<pool::ReadPool>,
}

impl Db {
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let conn = Connection::open(&path)?;
        pool::tune_writer(&conn)?;
        let mut db = Db {
            conn: Arc::new(Mutex::new(conn)),
            readers: Arc::new(pool::ReadPool::empty()),
        };
        db.init_schema()?;
        db.migrate()?;
        db.readers = Arc::new(pool::ReadPool::open(&path)?);
        // After the schema, so the -wal and -shm SQLite creates along the way are covered too.
        restrict_permissions(&path);
        Ok(db)
    }

    pub fn new_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let db = Db {
            conn: Arc::new(Mutex::new(conn)),
            readers: Arc::new(pool::ReadPool::empty()),
        };
        db.init_schema()?;
        db.migrate()?;
        Ok(db)
    }
}

impl Db {
    /// A connection for a method made entirely of SELECTs.
    ///
    /// Never call this while holding [`Self::conn`]: a reader cannot see the writer's uncommitted
    /// transaction, so a read inside a write would answer from before it.
    pub(crate) fn read(&self) -> MutexGuard<'_, Connection> {
        match self.readers.get() {
            Some(reader) => reader,
            None => self.conn.lock().unwrap(),
        }
    }
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
