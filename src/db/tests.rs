//! Tests for the core `Db` methods, one file per subject.

mod device_keys;
mod handoff;
mod jam_recaps;
mod migration_order;
mod node_naming;
#[cfg(unix)]
mod permissions;
mod read_pool;
mod retention;
mod scrobble_time;
mod sealed_notes;
mod settings_vault;
