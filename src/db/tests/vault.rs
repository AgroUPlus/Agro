//! The cloud vault keeps a few sealed backups per account, and only ever hands them to their owner.

use crate::db::*;
use crate::db_vault::{VaultLabel, VaultSection, KEEP_PER_ACCOUNT};

fn label(device: &str, sections: &[(&str, i64)]) -> VaultLabel {
    VaultLabel {
        device_id: device.into(),
        device_name: None,
        app_version: None,
        format: 1,
        plain_bytes: 100,
        sections: sections
            .iter()
            .map(|(n, c)| VaultSection {
                name: (*n).into(),
                count: *c,
            })
            .collect(),
    }
}

#[test]
fn the_newest_few_are_kept_and_the_oldest_go() {
    let db = Db::new_in_memory().unwrap();
    let mut ids = Vec::new();
    for i in 0..KEEP_PER_ACCOUNT + 2 {
        let stored = db
            .store_vault_backup("alpha", &label("phone", &[("SETTINGS", i)]), &[i as u8; 4])
            .unwrap();
        ids.push(stored.id);
        // `created_at` orders them; keep each one strictly later than the last.
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let kept: Vec<_> = db
        .vault_backups("alpha")
        .unwrap()
        .into_iter()
        .map(|b| b.id)
        .collect();
    let newest: Vec<_> = ids
        .iter()
        .rev()
        .take(KEEP_PER_ACCOUNT as usize)
        .cloned()
        .collect();
    assert_eq!(kept, newest);
}

#[test]
fn a_backup_is_only_ever_its_owners_and_comes_back_byte_for_byte() {
    let db = Db::new_in_memory().unwrap();
    let sealed = b"opaque sealed bytes".to_vec();
    let stored = db
        .store_vault_backup(
            "alpha",
            &label("phone", &[("SETTINGS", 4), ("ACCOUNTS", 2)]),
            &sealed,
        )
        .unwrap();
    assert!(stored.includes_accounts);
    assert_eq!(stored.sealed_bytes, sealed.len() as i64);

    assert!(db.vault_backup_blob("beta", &stored.id).unwrap().is_none());
    assert!(db.vault_backups("beta").unwrap().is_empty());
    assert!(!db.delete_vault_backup("beta", &stored.id).unwrap());

    let (label, bytes) = db.vault_backup_blob("alpha", &stored.id).unwrap().unwrap();
    assert_eq!(bytes, sealed);
    assert_eq!(label.sections.len(), 2);
    assert!(db.delete_vault_backup("alpha", &stored.id).unwrap());
    assert!(db.vault_backups("alpha").unwrap().is_empty());
}
