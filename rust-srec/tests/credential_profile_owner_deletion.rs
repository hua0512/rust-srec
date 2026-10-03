use std::time::Duration;

use rust_srec::credentials::{
    CredentialBinding, CredentialIdentity, CredentialMaterial, CredentialOwner,
    CredentialSelection, ResolvedCredentialPolicy,
};
use rust_srec::database::models::StreamerDbModel;
use rust_srec::database::repositories::{
    ConfigRepository, CredentialProfileRepository, SqlxConfigRepository, SqlxStreamerRepository,
    StreamerRepository,
};

#[tokio::test]
async fn every_owner_delete_preserves_active_binding_then_reaps_profiles_and_health() {
    tokio::time::timeout(Duration::from_secs(20), async {
        for kind in ["platform", "template", "streamer"] {
            let pool = rust_srec::database::init_pool_with_size("sqlite::memory:", 1).await.unwrap();
            rust_srec::database::run_migrations(&pool).await.unwrap();
            let config = SqlxConfigRepository::new(pool.clone(), pool.clone());
            let streamers = SqlxStreamerRepository::new(pool.clone(), pool.clone());
            let profiles = CredentialProfileRepository::new(pool.clone(), pool.clone());
            let owner = match kind {
                "platform" => CredentialOwner::Platform { platform_id: "platform-bilibili".into() },
                "template" => {
                    sqlx::query("INSERT INTO template_config(id, name) VALUES ('owner', 'Owner')").execute(&pool).await.unwrap();
                    CredentialOwner::Template { template_id: "owner".into() }
                }
                _ => {
                    let mut row = StreamerDbModel::new("Owner", "https://example.test/owner", "platform-bilibili");
                    row.id = "owner".into();
                    streamers.create_streamer(&row).await.unwrap();
                    CredentialOwner::Streamer { streamer_id: "owner".into() }
                }
            };
            let material = CredentialMaterial { cookies: "session=owner".into(), refresh_token: None, access_token: None, reauth_config: None };
            let account = profiles.create(&owner, "platform-bilibili", "Owner account", true, &material).await.unwrap();
            profiles.publish_health(&account, "valid", None, None, true).await.unwrap();
            let selection = CredentialSelection::Fixed { credential_id: account.id.clone() };
            // Same-owner policy references may disappear atomically with the owner.
            match kind {
                "platform" => { sqlx::query("UPDATE platform_config SET credential_selection = ? WHERE id = 'platform-bilibili'").bind(serde_json::to_string(&selection).unwrap()).execute(&pool).await.unwrap(); }
                "template" => { sqlx::query("UPDATE template_config SET platform_overrides = ? WHERE id = 'owner'").bind(serde_json::json!({"bilibili":{"credential_selection":selection}}).to_string()).execute(&pool).await.unwrap(); }
                _ => { sqlx::query("UPDATE streamers SET streamer_specific_config = ? WHERE id = 'owner'").bind(serde_json::json!({"credential_selection":selection}).to_string()).execute(&pool).await.unwrap(); }
            }
            let binding = CredentialBinding { identity: CredentialIdentity::Profile { profile_id: account.id.clone() }, revision: 1,
                policy: ResolvedCredentialPolicy::new("platform-bilibili".into(), owner.clone(), selection).unwrap(), epoch: 1 };
            sqlx::query("INSERT INTO live_sessions(id, streamer_name, start_time, credential_binding) VALUES ('active', 'Owner', 1, ?)").bind(serde_json::to_string(&binding).unwrap()).execute(&pool).await.unwrap();
            if kind == "streamer" { assert!(streamers.mark_streamer_deleted(owner.id()).await.unwrap()); }
            let blocked = match kind {
                "platform" => config.delete_platform_config(owner.id()).await,
                "template" => config.delete_template_config(owner.id()).await,
                _ => streamers.delete_marked_streamer(owner.id()).await.map(|_| ()),
            };
            assert!(blocked.is_err(), "{kind} deleted an actively bound profile");
            assert!(profiles.get(&account.id).await.is_ok());
            sqlx::query("UPDATE live_sessions SET end_time = 2 WHERE id = 'active'").execute(&pool).await.unwrap();
            match kind {
                "platform" => config.delete_platform_config(owner.id()).await.unwrap(),
                "template" => config.delete_template_config(owner.id()).await.unwrap(),
                _ => assert!(streamers.delete_marked_streamer(owner.id()).await.unwrap()),
            }
            assert!(profiles.get(&account.id).await.is_err());
            let health: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM credential_profile_health").fetch_one(&pool).await.unwrap();
            assert_eq!(health, 0);
            let retained: String = sqlx::query_scalar("SELECT credential_binding FROM live_sessions WHERE id = 'active'").fetch_one(&pool).await.unwrap();
            assert!(retained.contains(&account.id), "ended-session audit identity must survive");
            assert!(sqlx::query("PRAGMA foreign_key_check").fetch_all(&pool).await.unwrap().is_empty());
        }
    }).await.expect("owner deletion matrix must finish");
}
