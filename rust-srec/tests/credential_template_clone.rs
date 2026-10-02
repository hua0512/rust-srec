use std::time::Duration;

use rust_srec::credentials::{CredentialMaterial, CredentialOwner};
use rust_srec::database::models::TemplateConfigDbModel;
use rust_srec::database::repositories::{
    ConfigRepository, CredentialProfileRepository, SqlxConfigRepository,
};
use serde_json::json;

#[tokio::test]
async fn cloning_remaps_local_accounts_keeps_shared_ids_and_rolls_back_name_collisions() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let pool = rust_srec::database::init_pool_with_size("sqlite::memory:", 1).await.unwrap();
        rust_srec::database::run_migrations(&pool).await.unwrap();
        let configs = SqlxConfigRepository::new(pool.clone(), pool.clone());
        let profiles = CredentialProfileRepository::new(pool.clone(), pool.clone());
        let mut template = TemplateConfigDbModel::new("Original");
        configs.create_template_config(&template).await.unwrap();
        let owner = CredentialOwner::Template { template_id: template.id.clone() };
        let material = CredentialMaterial { cookies: "session=local".into(), refresh_token: Some("refresh=local".into()), access_token: None, reauth_config: None };
        let local = profiles.create(&owner, "platform-bilibili", "Local", true, &material).await.unwrap();
        let unused = profiles.create(&owner, "platform-bilibili", "Unused", false, &material).await.unwrap();
        let shared = profiles.create(&CredentialOwner::Platform { platform_id: "platform-bilibili".into() }, "platform-bilibili", "Shared", true, &material).await.unwrap();
        profiles.publish_health(&local, "invalid", None, Some("expired"), true).await.unwrap();
        template.platform_overrides = Some(json!({"bilibili":{"quality":120,"credential_selection":{"mode":"pool","credential_ids":[shared.id,local.id],"strategy":"priority","failover":true,"max_attempts":3}}, "soop":{"credential_selection":{"mode":"none"}}}).to_string());
        configs.update_template_config(&template).await.unwrap();
        let cloned = configs.clone_template_config(&template.id, "Cloned").await.unwrap();
        assert_ne!(cloned.id, template.id);
        let copies = profiles.accessible(&CredentialOwner::Template { template_id: cloned.id.clone() }, "platform-bilibili").await.unwrap();
        let local_copy = copies.iter().find(|p| p.label == "Local").unwrap();
        let unused_copy = copies.iter().find(|p| p.label == "Unused").unwrap();
        assert_ne!(local_copy.id, local.id);
        assert_ne!(unused_copy.id, unused.id);
        assert_eq!(local_copy.cookies, material.cookies);
        assert_eq!(local_copy.refresh_token, material.refresh_token);
        assert!(!unused_copy.enabled);
        assert_eq!(local_copy.revision, 1);
        assert!(profiles.health(&local_copy.id).await.unwrap().is_none());
        let overrides: serde_json::Value = serde_json::from_str(cloned.platform_overrides.as_deref().unwrap()).unwrap();
        assert_eq!(overrides["bilibili"]["credential_selection"]["credential_ids"], json!([shared.id,local_copy.id]));
        assert_eq!(overrides["bilibili"]["quality"], 120);
        assert_eq!(overrides["soop"]["credential_selection"], json!({"mode":"none"}));
        assert_eq!(configs.get_template_config(&template.id).await.unwrap().platform_overrides, template.platform_overrides);
        let before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM credential_profiles").fetch_one(&pool).await.unwrap();
        assert!(configs.clone_template_config(&template.id, "Cloned").await.is_err());
        let after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM credential_profiles").fetch_one(&pool).await.unwrap();
        assert_eq!(before, after);
        assert!(sqlx::query("PRAGMA foreign_key_check").fetch_all(&pool).await.unwrap().is_empty());
    }).await.expect("template clone must commit or roll back atomically");
}
