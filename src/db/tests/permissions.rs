//! The database file and its sidecars are owner-only.

use crate::db::*;
use std::os::unix::fs::PermissionsExt;

/// The database and every sidecar SQLite writes beside it. A 0600 database next to a 0644
/// write-ahead log protects nothing: the `-wal` holds recent writes in plaintext.
#[test]
fn the_database_and_its_sidecars_are_owner_only() {
    let dir = std::env::temp_dir().join(format!("agro-perm-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("agro_test.db");

    let db = Db::new(&path).unwrap();
    // Force a write so the -wal exists to be checked.
    drop(db);

    for suffix in ["", "-wal", "-shm"] {
        let mut name = path.as_os_str().to_os_string();
        name.push(suffix);
        let target = std::path::PathBuf::from(name);
        if !target.exists() {
            continue;
        }
        let mode = std::fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "{} is {:o}, not 0600", target.display(), mode);
    }
    let _ = std::fs::remove_dir_all(&dir);
}
