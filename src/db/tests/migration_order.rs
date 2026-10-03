//! Migrations are append-only, and every upgrade path reaches the same schema.

use crate::db::migrations::MIGRATIONS;
use crate::db::*;

/// The schema a database ends up with, as text, so two of them can be compared.
///
/// `user_version` is included because it is what decides whether anything runs at all: two
/// databases with identical tables but different stamps will diverge on the next upgrade.
fn shape(db: &Db) -> String {
    let conn = db.conn.lock().unwrap();
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    let mut stmt = conn
        .prepare("SELECT sql FROM sqlite_master WHERE sql IS NOT NULL ORDER BY name")
        .unwrap();
    let mut lines: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    // `ALTER TABLE ... ADD COLUMN` rewrites the stored CREATE statement in place, so column
    // additions show up here without needing to be listed separately.
    lines.sort();
    format!("v{version}\n{}", lines.join("\n"))
}

/// A database that stopped after `applied` migrations, the way a deployment mid-upgrade has.
fn stopped_after(applied: usize) -> Db {
    let conn = Connection::open_in_memory().unwrap();
    let db = Db {
        conn: Arc::new(Mutex::new(conn)),
        readers: Arc::new(crate::db::pool::ReadPool::empty()),
    };
    db.init_schema().unwrap();
    {
        let mut conn = db.conn.lock().unwrap();
        for (index, migration) in MIGRATIONS.iter().enumerate().take(applied) {
            let tx = conn.transaction().unwrap();
            tx.execute_batch(migration).unwrap();
            tx.execute_batch(&format!("PRAGMA user_version = {}", index + 1))
                .unwrap();
            tx.commit().unwrap();
        }
    }
    db
}

/// The digest of every migration that has shipped, in order.
///
/// Recorded rather than derived: a test that recomputes both sides from the same list can only
/// ever agree with itself, and would not notice the list being reordered under it. These are
/// the numbers a released Agro has already stamped into real databases.
///
/// **Appending an entry means appending a line here. Nothing else is a legal edit.** If a
/// digest below no longer matches, an entry was inserted, removed or rewritten in place, and
/// every database past that index will skip the new entry forever — see the test.
const SHIPPED: &[&str] = &[
    "3d5d6f5b2a582c1a",
    "3ae30aab8621167b",
    "2f0f62bb0ece4b14",
    "e4dc022c6497c3da",
    "bddcb87c38c0ae7f",
    "aee4ac3b5e2ed1e5",
    "6b8e9d354bf789cc",
    "805443a740e42efb",
    "fb72d6d909a9de17",
    "8fa8e99245e1b00f",
    "a6115db89f5b5d1e",
    "2c5dd4778258b85a",
    "9403140a0251c1b7",
    "e10d684b7b98f29e",
    "d5e792a12d7869ad",
    "93bf2072cf37588d",
    "01cff0d4e4f9b49c",
    "ff26008ad5d56145",
    "d9c76e00d66377f4",
    "2d10ecdaffce859e",
    "bceaa97fe736cc7e",
    "7c76f2359c4b642a",
    "5e0f4c0d555d9fb9",
    "6e2ef4efc19294e1",
    "ddac48675a1edc87",
    "c3469b9564c8c0ea",
    "039187171941ac12",
    "e833dad23a9cb7f1",
    "df465a790c78bdcd",
    "32c2c5b3af907296",
    "b8b8e6e093d65c38",
    "f2f78d5fd389c3db",
    "9e3052c5944a23dc",
    "f98cdcd601306df4",
    "8e45b726c59580fb",
    "212565ec442faf61",
    "d3ee4106c8147dc0",
    "a2cfb5f0854b9ecb",
    "34675f8123a3bd73",
    "77101b2e3546de9b",
    "b6ee10866b26ddbc",
    "137eac22d9cf2270",
    "32e9dd64f1769710",
    "873725f41d4130d2",
    // The two lyrics columns shipped without a line here, which left them unpinned: an entry
    // inserted before either of them would have renumbered it with nothing to notice.
    "1d4f6a273e4bcf5b",
    "d5bc8bb460c3e9c5",
    "549e6b2be0e83b04",
    "7ba7fb712ebb777d",
    "0843bda0607e7dc0",
    "b57e6f29183a7c68",
    // Short-link reuse. Shipped on `main` as 50 and moved here: see the entry's own comment.
    "722e6f5d40e2e152",
];

fn digest(migration: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(migration.as_bytes());
    format!("{:x}", hasher.finalize())[..16].to_string()
}

/// [`MIGRATIONS`] is append-only, and this is what enforces it.
///
/// `migrate` stamps `PRAGMA user_version` from an entry's *index*, and skips everything at or
/// below the stamp it finds. So an entry inserted anywhere but the end renumbers every entry
/// after it, onto numbers that have already been stamped in the field: those databases skip the
/// new entry permanently and re-run its neighbour instead. The column never appears, and every
/// query naming it fails at runtime.
///
/// No other test in this repo can see it. They all build a database from scratch, and a fresh
/// database applies the whole list in order whatever order that is. The bug exists only on a
/// database that has been running — which is to say, only in production.
#[test]
fn migrations_are_append_only() {
    for (index, shipped) in SHIPPED.iter().enumerate() {
        let current = MIGRATIONS.get(index).unwrap_or_else(|| {
            panic!(
                "migration {index} has been removed. Databases stamped past it will never \
                 run what replaced it."
            )
        });
        assert_eq!(
            &digest(current),
            shipped,
            "migration {index} is not the one that shipped as version {}. It was inserted, \
             reordered or edited in place; every database already past this version will skip \
             it and re-run its neighbour instead. Append instead, and add a line to SHIPPED.",
            index + 1
        );
    }
}

/// Upgrading from any point in the list must arrive at the schema a fresh install has.
///
/// Weaker than the digest pin above and complementary to it: this one catches a migration that
/// is in the right place but does not do what the initial schema does — a table created by
/// `init_schema` for new databases and never added by a migration for old ones.
#[test]
fn upgrading_from_any_earlier_version_reaches_the_same_schema() {
    let fresh = shape(&Db::new_in_memory().unwrap());

    for applied in 0..=MIGRATIONS.len() {
        let db = stopped_after(applied);
        db.migrate().unwrap();
        assert_eq!(
            shape(&db),
            fresh,
            "a database that had applied {applied} migrations did not reach the current schema"
        );
    }
}

/// The column the sealed handoff depends on, checked by name on an upgraded database.
///
/// Named separately from the comparison above so a failure says what broke rather than
/// printing two schemas and leaving the reader to diff them.
#[test]
fn a_running_database_gains_the_sealed_handoff_columns() {
    let db = stopped_after(MIGRATIONS.len() - 1);
    db.migrate().unwrap();
    let conn = db.conn.lock().unwrap();
    let sql: String = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name = 'handoff_state'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        sql.contains("encrypted_payload"),
        "an upgraded database has no encrypted_payload column: {sql}"
    );
    let sealed: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name = 'handoff_presence_ciphertexts'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(sealed, 1, "an upgraded database has no presence copy table");
}
