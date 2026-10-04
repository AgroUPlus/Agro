//! What the dashboard and Wanda ask of the vault API, field for field.
#![cfg(test)]

use crate::blend_boundary_tests::harness;
use crate::db_vault::{VaultLabel, VaultSection};

/// The dashboard's Backups tab query, as `BackupsTab.jsx` sends it.
#[tokio::test]
async fn the_server_answers_what_the_dashboard_and_wanda_ask() {
    let h = harness();
    let label = VaultLabel {
        device_id: "phone".into(),
        device_name: Some("Pixel".into()),
        app_version: Some("1.5.5".into()),
        format: 1,
        plain_bytes: 1000,
        sections: vec![VaultSection {
            name: "SETTINGS".into(),
            count: 4,
        }],
    };
    h.db.store_vault_backup("alpha", &label, b"sealed").unwrap();

    let listed = h
        .ok(
            &h.alpha,
            "query Backups {
               vaultBackups {
                 id createdAt deviceId deviceName appVersion plainBytes sealedBytes includesAccounts sha256
                 sections { name count }
               }
               vaultLimits { keepPerAccount maxSealedBytes }
             }",
        )
        .await;
    assert_eq!(listed["vaultBackups"][0]["deviceName"], "Pixel");
    assert_eq!(listed["vaultBackups"][0]["sections"][0]["count"], 4);

    let theirs = h.ok(&h.beta, "{ vaultBackups { id } }").await;
    assert_eq!(
        theirs["vaultBackups"],
        serde_json::json!([]),
        "another account's backup was listed"
    );

    let id = listed["vaultBackups"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let stolen = h
        .ok(
            &h.beta,
            &format!(r#"mutation {{ deleteVaultBackup(id: "{id}") }}"#),
        )
        .await;
    assert_eq!(stolen["deleteVaultBackup"], false);
    assert_eq!(h.db.vault_backups("alpha").unwrap().len(), 1);
}
