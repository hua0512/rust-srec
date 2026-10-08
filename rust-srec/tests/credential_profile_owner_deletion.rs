use std::time::Duration;

use rust_srec::credentials::{
    CredentialBinding, CredentialIdentity, CredentialMaterial, CredentialOwner,
    CredentialSelection, CredentialValidity, ResolvedCredentialPolicy,
};
use rust_srec::database::models::StreamerDbModel;
use rust_srec::database::repositories::{
    ConfigRepository, CredentialProfileRepository, SqlxConfigRepository, SqlxStreamerRepository,
    StreamerRepository,
};

fn material() -> CredentialMaterial {
    CredentialMaterial {
        cookies: "session=owner".into(),
        refresh_token: None,
        access_token: None,
        reauth_config: None,
    }
}

/// Profiles belong to the platform: removing a template or streamer that
/// selected one leaves the account in place.
#[tokio::test]
async fn deleting_a_selecting_template_or_streamer_keeps_the_platform_profile() {
    tokio::time::timeout(Duration::from_secs(20), async {
        let pool = rust_srec::database::init_pool_with_size("sqlite::memory:", 1)
            .await
            .unwrap();
        rust_srec::database::run_migrations(&pool).await.unwrap();
        let config = SqlxConfigRepository::new(pool.clone(), pool.clone());
        let streamers = SqlxStreamerRepository::new(pool.clone(), pool.clone());
        let profiles = CredentialProfileRepository::new(pool.clone(), pool.clone());
        let account = profiles
            .create(
                "platform-bilibili",
                "Shared",
                true,
                &material(),
                &rust_srec::proxies::ProxyRoute::Inherit,
            )
            .await
            .unwrap();
        let selection = serde_json::json!({"mode": "fixed", "credential_id": account.id});
        let mut template = rust_srec::database::models::TemplateConfigDbModel::new("Owner");
        template.id = "owner".into();
        template.platform_overrides =
            Some(serde_json::json!({"bilibili": {"credential_selection": selection}}).to_string());
        config.create_template_config(&template).await.unwrap();
        let mut row =
            StreamerDbModel::new("Owner", "https://example.test/owner", "platform-bilibili");
        row.id = "owner".into();
        row.streamer_specific_config =
            Some(serde_json::json!({"credential_selection": selection}).to_string());
        streamers.create_streamer(&row).await.unwrap();

        assert_eq!(
            profiles
                .references(&account.id)
                .await
                .unwrap()
                .selections
                .len(),
            2
        );
        config.delete_template_config("owner").await.unwrap();
        assert!(streamers.mark_streamer_deleted("owner").await.unwrap());
        assert!(streamers.delete_marked_streamer("owner").await.unwrap());
        assert!(profiles.get(&account.id).await.is_ok());
        // Neither owner selects it any more.
        assert!(profiles.references(&account.id).await.unwrap().is_empty());
        assert!(
            sqlx::query("PRAGMA foreign_key_check")
                .fetch_all(&pool)
                .await
                .unwrap()
                .is_empty()
        );
    })
    .await
    .expect("deletion must finish");
}

#[tokio::test]
async fn platform_delete_waits_for_active_binding_then_removes_its_profiles_and_health() {
    tokio::time::timeout(Duration::from_secs(20), async {
        let pool = rust_srec::database::init_pool_with_size("sqlite::memory:", 1)
            .await
            .unwrap();
        rust_srec::database::run_migrations(&pool).await.unwrap();
        let config = SqlxConfigRepository::new(pool.clone(), pool.clone());
        let profiles = CredentialProfileRepository::new(pool.clone(), pool.clone());
        let owner = CredentialOwner::Platform {
            platform_id: "platform-bilibili".into(),
        };
        let account = profiles
            .create("platform-bilibili", "Owner account", true, &material(), &rust_srec::proxies::ProxyRoute::Inherit)
            .await
            .unwrap();
        profiles
            .publish_health(&account, CredentialValidity::Valid, None, true)
            .await
            .unwrap();
        let selection = CredentialSelection::Fixed {
            credential_id: account.id.clone(),
        };
        // The platform's own policy disappears with it.
        config
            .update_platform_config_with_selection(
                &config.get_platform_config("platform-bilibili").await.unwrap(),
                Some(&selection),
            )
            .await
            .unwrap();
        let binding = CredentialBinding {
            identity: CredentialIdentity::Profile {
                profile_id: account.id.clone(),
            },
            revision: 1,
            policy: ResolvedCredentialPolicy::new("platform-bilibili".into(), owner, selection)
                .unwrap(),
            epoch: 1,
        };
        sqlx::query("INSERT INTO live_sessions(id, streamer_name, start_time, credential_binding) VALUES ('active', 'Owner', 1, ?)")
            .bind(serde_json::to_string(&binding).unwrap())
            .execute(&pool)
            .await
            .unwrap();

        assert!(
            config.delete_platform_config("platform-bilibili").await.is_err(),
            "a platform with an actively bound profile was deleted"
        );
        assert!(profiles.get(&account.id).await.is_ok());

        sqlx::query("UPDATE live_sessions SET end_time = 2 WHERE id = 'active'")
            .execute(&pool)
            .await
            .unwrap();
        config
            .delete_platform_config("platform-bilibili")
            .await
            .unwrap();
        assert!(profiles.get(&account.id).await.is_err());
        let health: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM credential_profile_health")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(health, 0);
        let retained: String =
            sqlx::query_scalar("SELECT credential_binding FROM live_sessions WHERE id = 'active'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(
            retained.contains(&account.id),
            "ended-session audit identity must survive"
        );
        assert!(
            sqlx::query("PRAGMA foreign_key_check")
                .fetch_all(&pool)
                .await
                .unwrap()
                .is_empty()
        );
    })
    .await
    .expect("platform deletion must finish");
}
