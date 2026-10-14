//! Spool eviction.

use super::*;

#[test]
fn spool_evicts_oldest_first_when_over_budget() {
    let db = Db::new_in_memory().unwrap();
    for (hash, size) in [("a", 100), ("b", 100), ("c", 100)] {
        db.spool_insert(hash, size, "laptop", "alpha", 72).unwrap();
        // created_at has second resolution in RFC3339, so order the inserts explicitly.
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert_eq!(db.spool_total_bytes().unwrap(), 300);

    let doomed = db.spool_evictable_for("alpha", 150).unwrap();
    assert_eq!(doomed.len(), 2, "two must go to get under 150");
    assert_eq!(doomed[0].0, "a", "oldest first");
}

/// One account filling the spool used to evict another's staged files: the budget was global
/// and eviction ran oldest-first across everybody.
#[test]
fn spool_eviction_never_reaches_another_account() {
    let db = Db::new_in_memory().unwrap();
    db.spool_insert("admin-file", 100, "desktop", "alpha", 72)
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2));
    for (hash, size) in [("guest-1", 100), ("guest-2", 100)] {
        db.spool_insert(hash, size, "phone", "guest", 72).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }

    let doomed = db.spool_evictable_for("guest", 100).unwrap();
    assert!(
        doomed.iter().all(|(hash, _)| hash.starts_with("guest")),
        "a guest's eviction reached the admin's spool: {doomed:?}"
    );
    assert_eq!(db.spool_bytes_for("alpha").unwrap(), 100);
    assert_eq!(db.spool_bytes_for("guest").unwrap(), 200);
}

#[test]
fn spool_keeps_everything_when_under_budget() {
    let db = Db::new_in_memory().unwrap();
    db.spool_insert("a", 100, "laptop", "alpha", 72).unwrap();
    assert!(db.spool_evictable_for("alpha", 1_000).unwrap().is_empty());
}
