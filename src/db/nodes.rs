//! Registered devices: the fleet an account can hand playback between.

use rusqlite::{params, Result};

use super::Db;

impl Db {
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
        let conn = self.read();
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
        let conn = self.read();
        let mut stmt = conn.prepare(
            "SELECT device_id, user_id, petname, client_type, version, current_track, last_seen_at
             FROM registered_nodes WHERE user_id = ?1 ORDER BY last_seen_at DESC",
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
