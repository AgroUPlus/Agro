//! Which plugins the operator has switched on.

use rusqlite::{params, Result};

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
