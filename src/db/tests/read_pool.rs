//! Readers on a file-backed database see what the writer committed.

use crate::db::*;
use crate::db_identity::{AccountState, Role};

fn file_db(name: &str) -> (Db, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("agro-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    (Db::new(dir.join("agro_test.db")).unwrap(), dir)
}

/// WAL is what lets a reader run beside the writer at all; without it the pool would only add
/// connections that wait on each other.
#[test]
fn the_file_runs_in_wal_mode() {
    let (db, dir) = file_db("wal");
    let mode: String = db
        .read()
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .unwrap();
    assert_eq!(mode.to_lowercase(), "wal");
    drop(db);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A token minted on the writer resolves through a reader straight away. This is the check every
/// authenticated request makes, so a stale reader would sign people out.
#[test]
fn a_reader_sees_the_writers_commits() {
    let (db, dir) = file_db("readers");
    db.create_account(
        "alpha",
        "correct horse battery",
        Role::Member,
        AccountState::Active,
    )
    .unwrap();
    let token = db.mint_device_token("alpha", "phone").unwrap();

    assert!(db.account("alpha").unwrap().is_some());
    let (account, label) = db.account_for_token(&token).unwrap().unwrap();
    assert_eq!(account.username.to_string(), "alpha");
    assert_eq!(label, "phone");
    drop(db);
    let _ = std::fs::remove_dir_all(&dir);
}
