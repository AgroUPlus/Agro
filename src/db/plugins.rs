//! Which plugins the operator has switched on.

use rusqlite::{params, OptionalExtension, Result};

use super::Db;

impl Db {
    pub fn set_plugin_enabled(&self, plugin_id: &str, is_enabled: bool) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO plugins_state (id, is_enabled) VALUES (?1, ?2)
             ON CONFLICT(id) DO UPDATE SET is_enabled = excluded.is_enabled",
            params![plugin_id, is_enabled],
        )?;
        Ok(())
    }

    /// One plugin's saved state, or `None` when nobody has ever switched it. Read on hot paths —
    /// every heartbeat asks about presence — so it is a primary-key lookup on a reader.
    pub fn plugin_state(&self, plugin_id: &str) -> Result<Option<bool>> {
        self.read()
            .query_row(
                "SELECT is_enabled FROM plugins_state WHERE id = ?1",
                params![plugin_id],
                |row| row.get(0),
            )
            .optional()
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
}
