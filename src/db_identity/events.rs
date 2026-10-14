//! The security event log.

use super::*;
use crate::lock::LockExt;
use rusqlite::{params, Result};

impl Db {
    /// Appends one event to the security log.
    ///
    /// Best-effort and infallible by design: a failure is logged and swallowed. The alternative is
    /// that a full disk or a locked database turns every login into a 500, which is a worse outcome
    /// than a gap in the record. Callers therefore do not have to decide what to do when auditing
    /// fails, which means they will not decide wrongly.
    pub fn record_event(&self, event: crate::audit::Event, record: crate::audit::Record) {
        let conn = self.conn.lock_recover();
        let result = conn.execute(
            "INSERT INTO security_events (at, user_id, kind, client_ip, device_label, detail)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                chrono::Utc::now().to_rfc3339(),
                record.user_id,
                event.as_str(),
                record.client_ip,
                record.device_label,
                record.detail,
            ],
        );
        if let Err(e) = result {
            tracing::warn!("could not record security event {}: {e}", event.as_str());
        }
    }

    /// The most recent events, newest first.
    ///
    /// `username` scopes to one account; `None` is the server-wide view and is admin-only at the
    /// resolver. `limit` is clamped, because an audit view is not a bulk export.
    pub fn security_events(
        &self,
        username: Option<&str>,
        limit: i64,
    ) -> Result<Vec<SecurityEvent>> {
        let limit = limit.clamp(1, 500);
        let conn = self.read();
        let read = |sql: &str, args: &[&dyn rusqlite::ToSql]| -> Result<Vec<SecurityEvent>> {
            let mut stmt = conn.prepare(sql)?;
            let rows = stmt
                .query_map(args, |row| {
                    Ok(SecurityEvent {
                        id: row.get(0)?,
                        at: row.get(1)?,
                        user_id: row.get(2)?,
                        kind: row.get(3)?,
                        client_ip: row.get(4)?,
                        device_label: row.get(5)?,
                        detail: row.get(6)?,
                    })
                })?
                .collect::<Result<Vec<_>>>()?;
            Ok(rows)
        };

        match username {
            Some(name) => read(
                "SELECT id, at, user_id, kind, client_ip, device_label, detail
                   FROM security_events
                  WHERE user_id = ?1 COLLATE NOCASE
                  ORDER BY at DESC, id DESC LIMIT ?2",
                params![name.trim(), limit],
            ),
            None => read(
                "SELECT id, at, user_id, kind, client_ip, device_label, detail
                   FROM security_events
                  ORDER BY at DESC, id DESC LIMIT ?1",
                params![limit],
            ),
        }
    }

    /// Drops events past the retention window.
    ///
    /// An audit log that grows forever is the privacy liability it was meant to prevent. Runs on
    /// the same sweeper as everything else.
    pub fn sweep_security_events(&self) -> Result<usize> {
        let days = std::env::var("AGRO_AUDIT_RETENTION_DAYS")
            .ok()
            .and_then(|v| v.trim().parse::<i64>().ok())
            .filter(|d| *d > 0)
            .unwrap_or(180);
        let cutoff = (chrono::Utc::now() - chrono::Duration::days(days)).to_rfc3339();
        let conn = self.conn.lock_recover();
        conn.execute("DELETE FROM security_events WHERE at < ?1", params![cutoff])
    }
}
