//! Read-only connections beside the single writer.
//!
//! Every query used to queue on one mutex, so a slow write — a library import, the retention
//! sweep — held up every token check behind it. In WAL mode SQLite lets readers run alongside the
//! writer, and this pool is how that concurrency is reached.
//!
//! Writes stay on [`super::Db::conn`]. A method that writes holds that lock for its whole body,
//! exactly as before, so a read-then-write sequence is no less atomic than it was. Only methods
//! made entirely of SELECTs read through here.

use rusqlite::{Connection, OpenFlags, Result};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};

/// How many readers to open when `AGRO_DB_READERS` is unset.
const DEFAULT_READERS: usize = 4;

/// Applied to the writer. WAL is a property of the file, so setting it once here is what lets the
/// readers open alongside; it is not repeated on them.
///
/// `synchronous=NORMAL` is the documented pairing for WAL: a power cut can lose the last few
/// commits but never corrupts the file. `busy_timeout` makes a reader caught by a checkpoint wait
/// rather than fail.
pub(super) fn tune_writer(conn: &Connection) -> Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    tune_common(conn)
}

fn tune_common(conn: &Connection) -> Result<()> {
    conn.pragma_update(None, "busy_timeout", 5000)?;
    // Negative means KiB: 16 MiB of page cache per connection.
    conn.pragma_update(None, "cache_size", -16000)?;
    conn.pragma_update(None, "temp_store", "MEMORY")
}

pub(crate) struct ReadPool {
    slots: Vec<Mutex<Connection>>,
    next: AtomicUsize,
}

impl ReadPool {
    /// No readers: every read falls back to the writer. What an in-memory database gets, since a
    /// second connection to `:memory:` would be a second, empty database.
    pub(super) fn empty() -> Self {
        ReadPool {
            slots: Vec::new(),
            next: AtomicUsize::new(0),
        }
    }

    /// Opens the readers. Call after migrations, so they never see a half-built schema.
    pub(super) fn open(path: &Path) -> Result<Self> {
        // `Db::new(":memory:")` is an in-memory database too, and gets no readers for the same
        // reason `new_in_memory` does.
        if path.as_os_str() == ":memory:" {
            return Ok(Self::empty());
        }
        let count = std::env::var("AGRO_DB_READERS")
            .ok()
            .and_then(|v| v.trim().parse::<usize>().ok())
            .unwrap_or(DEFAULT_READERS);
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_URI;
        let slots = (0..count)
            .map(|_| {
                let conn = Connection::open_with_flags(path, flags)?;
                tune_common(&conn)?;
                Ok(Mutex::new(conn))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(ReadPool {
            slots,
            next: AtomicUsize::new(0),
        })
    }

    /// A free reader if there is one, otherwise a wait on the next in turn. `None` when the pool
    /// is empty, and the caller reads through the writer instead.
    pub(super) fn get(&self) -> Option<MutexGuard<'_, Connection>> {
        if self.slots.is_empty() {
            return None;
        }
        for slot in &self.slots {
            if let Ok(guard) = slot.try_lock() {
                return Some(guard);
            }
        }
        let turn = self.next.fetch_add(1, Ordering::Relaxed) % self.slots.len();
        Some(self.slots[turn].lock().unwrap())
    }
}
