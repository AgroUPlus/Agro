//! Schema changes, in order, and the code that applies them.
//!
//! The list is long enough to be split across files, by version. **Append only**: a new entry goes
//! at the end of the last file (or a new file after it, added to [`MIGRATIONS`] below), never
//! anywhere else. `tests::migration_order` pins every shipped entry by digest and fails on any other
//! edit.

use std::sync::LazyLock;

use rusqlite::Result;

use super::Db;

mod v01_09;
mod v10_18;
mod v19_30;
mod v31_41;
mod v42_49;

/// Schema changes, in order. **Append only** — an entry's index is its version number, so
/// reordering or removing one silently skips it on every database that has already run it.
pub(super) static MIGRATIONS: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    [
        v01_09::ENTRIES,
        v10_18::ENTRIES,
        v19_30::ENTRIES,
        v31_41::ENTRIES,
        v42_49::ENTRIES,
    ]
    .concat()
});

impl Db {
    /// Brings the database up to date.
    ///
    /// Two mechanisms, for two eras. [`Self::migrate_handoff_queue`] predates any version stamp
    /// and stays idempotent because databases exist in both states. Everything since is a numbered
    /// entry in [`MIGRATIONS`], applied in order, each in its own transaction, with
    /// `PRAGMA user_version` stamped as it goes — so each runs exactly once and a failure aborts
    /// startup rather than leaving a half-migrated database serving requests.
    pub(super) fn migrate(&self) -> Result<()> {
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
        let _ = conn.execute(
            "ALTER TABLE handoff_state ADD COLUMN queue_index INTEGER",
            [],
        );
    }
}
