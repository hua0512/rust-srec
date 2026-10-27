use rust_srec::credentials::{
    CredentialMaterial, CredentialOwner, CredentialSelection, CredentialValidity, HealthReason,
};
use rust_srec::database::repositories::{
    ConfigRepository, CredentialProfileRepository, SqlxConfigRepository,
};

async fn fixture() -> (sqlx::SqlitePool, CredentialProfileRepository) {
    let pool = rust_srec::database::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    rust_srec::database::run_migrations(&pool).await.unwrap();
    let repository = CredentialProfileRepository::new(pool.clone(), pool.clone());
    (pool, repository)
}

fn material(cookies: &str) -> CredentialMaterial {
    CredentialMaterial {
        cookies: cookies.to_string(),
        refresh_token: Some("refresh-sentinel".to_string()),
        access_token: None,
        reauth_config: None,
    }
}

#[tokio::test]
async fn profile_edits_separate_version_from_auth_revision_and_reject_stale_refresh() {
    let (_pool, repository) = fixture().await;
    let owner = CredentialOwner::Platform {
        platform_id: "platform-bilibili".to_string(),
    };
    let profile = repository
        .create(
            owner.id(),
            " A ",
            true,
            &material("sid=secret"),
            &rust_srec::proxies::ProxyRoute::Inherit,
        )
        .await
        .unwrap();
    let renamed = repository
        .update(&profile.id, profile.version, Some("B"), None, None, None)
        .await
        .unwrap();
    assert_eq!(renamed.revision, profile.revision);
    assert_eq!(renamed.version, profile.version + 1);
    assert!(
        repository
            .update(
                &profile.id,
                profile.version,
                Some("stale"),
                None,
                None,
                None
            )
            .await
            .is_err()
    );
    let disabled = repository
        .update(&profile.id, renamed.version, None, Some(false), None, None)
        .await
        .unwrap();
    assert_eq!(disabled.revision, profile.revision + 1);
    assert!(
        repository
            .publish_health(
                &profile,
                CredentialValidity::Invalid,
                Some(HealthReason::LoginRequired),
                true,
            )
            .await
            .is_err()
    );
    let replacement = CredentialMaterial {
        refresh_token: None,
        ..material("sid=new")
    };
    let replaced = repository
        .update(
            &profile.id,
            disabled.version,
            None,
            Some(true),
            Some(&replacement),
            None,
        )
        .await
        .unwrap();
    assert_eq!(replaced.refresh_token, None);
    assert!(!format!("{replaced:?}").contains("sid=new"));
    assert!(
        !serde_json::to_string(&replaced.summary())
            .unwrap()
            .contains("sid=new")
    );
}

#[tokio::test]
async fn explicit_policy_reference_blocks_profile_delete_and_owner_delete_removes_own_profiles() {
    let (pool, repository) = fixture().await;
    let owner = CredentialOwner::Platform {
        platform_id: "platform-bilibili".to_string(),
    };
    let profile = repository
        .create(
            owner.id(),
            "A",
            true,
            &material("sid=a"),
            &rust_srec::proxies::ProxyRoute::Inherit,
        )
        .await
        .unwrap();
    let selection = CredentialSelection::Fixed {
        credential_id: profile.id.clone(),
    };
    let config = SqlxConfigRepository::new(pool.clone(), pool.clone());
    let platform = config.get_platform_config(owner.id()).await.unwrap();
    config
        .update_platform_config_with_selection(&platform, Some(&selection))
        .await
        .unwrap();
    let error = repository
        .delete(&profile.id, profile.version)
        .await
        .unwrap_err();
    assert!(
        matches!(
            &error,
            rust_srec::Error::CredentialProfile(rust_srec::credentials::ProfileError::Referenced(references))
                if references.recordings.is_empty()
                    && references.selections.len() == 1
                    && references.selections[0].owner == owner
        ),
        "{error}"
    );
    config.delete_platform_config(owner.id()).await.unwrap();
    let selections: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM credential_selections")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(selections, 0);
    assert!(repository.get(&profile.id).await.is_err());
    let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(integrity, "ok");
    let violations = sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert!(violations.is_empty());
}

#[tokio::test]
async fn omitted_template_and_streamer_selections_stay_unchanged_until_explicitly_reset() {
    use rust_srec::database::models::{StreamerDbModel, TemplateConfigDbModel};
    use rust_srec::database::repositories::{SqlxStreamerRepository, StreamerRepository};

    let (pool, repository) = fixture().await;
    let owner = CredentialOwner::Platform {
        platform_id: "platform-bilibili".to_string(),
    };
    let profile = repository
        .create(
            owner.id(),
            "A",
            true,
            &material("sid=a"),
            &rust_srec::proxies::ProxyRoute::Inherit,
        )
        .await
        .unwrap();
    let fixed = serde_json::json!({"mode": "fixed", "credential_id": profile.id});
    let config = SqlxConfigRepository::new(pool.clone(), pool.clone());
    let mut template = TemplateConfigDbModel::new("Managed");
    template.platform_overrides = Some(
        serde_json::json!({"bilibili": {"credential_selection": fixed, "quality": 1}}).to_string(),
    );
    config.create_template_config(&template).await.unwrap();
    let overrides = |template: &TemplateConfigDbModel| -> serde_json::Value {
        serde_json::from_str(template.platform_overrides.as_deref().unwrap()).unwrap()
    };

    let template_owner = CredentialOwner::Template {
        template_id: template.id.clone(),
    };
    let selected = |stored: Vec<rust_srec::database::repositories::StoredSelection>| {
        stored
            .into_iter()
            .map(|stored| serde_json::to_value(stored.selection).unwrap())
            .collect::<Vec<_>>()
    };

    for replacement in [None, Some(serde_json::json!({"bilibili": {"quality": 2}}))] {
        template.platform_overrides = replacement.as_ref().map(ToString::to_string);
        config.update_template_config(&template).await.unwrap();
        let stored = config.get_template_config(&template.id).await.unwrap();
        assert_eq!(
            stored
                .platform_overrides
                .as_deref()
                .map(|raw| serde_json::from_str::<serde_json::Value>(raw).unwrap()),
            replacement
        );
        assert_eq!(
            selected(
                config
                    .credential_selections_for(&template_owner)
                    .await
                    .unwrap()
            ),
            vec![fixed.clone()]
        );
    }
    template.platform_overrides = Some(
        serde_json::json!({"bilibili": {"credential_selection": {"mode": "inherit"}}}).to_string(),
    );
    config.update_template_config(&template).await.unwrap();
    let stored = config.get_template_config(&template.id).await.unwrap();
    assert_eq!(overrides(&stored), serde_json::json!({"bilibili": {}}));
    assert!(
        config
            .credential_selections_for(&template_owner)
            .await
            .unwrap()
            .is_empty()
    );

    let streamers = SqlxStreamerRepository::new(pool.clone(), pool.clone());
    let mut streamer = StreamerDbModel::new(
        "Managed",
        "https://live.bilibili.com/1",
        "platform-bilibili",
    );
    streamer.streamer_specific_config =
        Some(serde_json::json!({"credential_selection": fixed}).to_string());
    streamers.create_streamer(&streamer).await.unwrap();
    streamer.streamer_specific_config = Some(serde_json::json!({"quality": 3}).to_string());
    streamers.update_streamer(&streamer).await.unwrap();
    let stored = streamers.get_streamer(&streamer.id).await.unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(
            stored.streamer_specific_config.as_deref().unwrap()
        )
        .unwrap(),
        serde_json::json!({"quality": 3})
    );
    let streamer_owner = CredentialOwner::Streamer {
        streamer_id: streamer.id.clone(),
    };
    assert_eq!(
        selected(
            config
                .credential_selections_for(&streamer_owner)
                .await
                .unwrap()
        ),
        vec![fixed]
    );
}

#[tokio::test]
async fn selections_name_only_their_platform_and_material_must_be_header_safe() {
    use rust_srec::database::models::{StreamerDbModel, TemplateConfigDbModel};
    use rust_srec::database::repositories::{SqlxStreamerRepository, StreamerRepository};

    let (pool, repository) = fixture().await;
    let profile = repository
        .create(
            "platform-bilibili",
            "A",
            true,
            &material("sid=a"),
            &rust_srec::proxies::ProxyRoute::Inherit,
        )
        .await
        .unwrap();
    let fixed = serde_json::json!({"mode": "fixed", "credential_id": profile.id});
    let streamers = SqlxStreamerRepository::new(pool.clone(), pool.clone());
    let mut foreign = StreamerDbModel::new("Huya", "https://www.huya.com/1", "platform-huya");
    foreign.streamer_specific_config =
        Some(serde_json::json!({"credential_selection": fixed}).to_string());
    assert!(streamers.create_streamer(&foreign).await.is_err());
    assert!(
        repository
            .create(
                "platform-bilibili",
                "bad",
                true,
                &material("sid=a\r\nHost: injected"),
                &rust_srec::proxies::ProxyRoute::Inherit
            )
            .await
            .is_err()
    );

    // A template that selected the account goes; the platform's account stays.
    let config = SqlxConfigRepository::new(pool.clone(), pool.clone());
    let mut template = TemplateConfigDbModel::new("Selecting");
    template.platform_overrides =
        Some(serde_json::json!({"bilibili": {"credential_selection": fixed}}).to_string());
    config.create_template_config(&template).await.unwrap();
    config.delete_template_config(&template.id).await.unwrap();
    assert!(repository.get(&profile.id).await.is_ok());
}
