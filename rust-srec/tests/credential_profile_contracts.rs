use rust_srec::credentials::{CredentialMaterial, CredentialOwner, CredentialSelection};
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
        .create(&owner, owner.id(), " A ", true, &material("sid=secret"))
        .await
        .unwrap();
    let renamed = repository
        .update(&profile.id, profile.version, Some("B"), None, None)
        .await
        .unwrap();
    assert_eq!(renamed.revision, profile.revision);
    assert_eq!(renamed.version, profile.version + 1);
    assert!(
        repository
            .update(&profile.id, profile.version, Some("stale"), None, None)
            .await
            .is_err()
    );
    let disabled = repository
        .update(&profile.id, renamed.version, None, Some(false), None)
        .await
        .unwrap();
    assert_eq!(disabled.revision, profile.revision + 1);
    assert!(
        repository
            .publish_health(&profile, "invalid", None, Some("expired"), true)
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
        )
        .await
        .unwrap();
    assert_eq!(replaced.refresh_token, None);
    assert!(!format!("{replaced:?}").contains("sid=new"));
    assert!(
        !serde_json::to_string(&replaced.summary().unwrap())
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
        .create(&owner, owner.id(), "A", true, &material("sid=a"))
        .await
        .unwrap();
    let selection = CredentialSelection::Fixed {
        credential_id: profile.id.clone(),
    };
    sqlx::query("UPDATE platform_config SET credential_selection = ? WHERE id = ?")
        .bind(serde_json::to_string(&selection).unwrap())
        .bind(owner.id())
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        repository
            .delete(&profile.id, profile.version)
            .await
            .is_err()
    );
    let config = SqlxConfigRepository::new(pool.clone(), pool.clone());
    config.delete_platform_config(owner.id()).await.unwrap();
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
async fn ownership_and_header_validation_prevent_cross_account_material() {
    let (pool, repository) = fixture().await;
    sqlx::query(
        "INSERT INTO template_config(id, name) VALUES ('template-a', 'A'), ('template-b', 'B')",
    )
    .execute(&pool)
    .await
    .unwrap();
    let owner = CredentialOwner::Template {
        template_id: "template-a".to_string(),
    };
    let profile = repository
        .create(&owner, "platform-bilibili", "A", true, &material("sid=a"))
        .await
        .unwrap();
    let other = CredentialOwner::Template {
        template_id: "template-b".to_string(),
    };
    assert!(
        repository
            .accessible(&other, "platform-bilibili")
            .await
            .unwrap()
            .iter()
            .all(|candidate| candidate.id != profile.id)
    );
    assert!(
        repository
            .create(
                &owner,
                "platform-bilibili",
                "bad",
                true,
                &material("sid=a\r\nHost: injected")
            )
            .await
            .is_err()
    );
    let config = SqlxConfigRepository::new(pool.clone(), pool.clone());
    assert!(
        config
            .delete_platform_config("platform-bilibili")
            .await
            .is_err()
    );
    config.delete_template_config("template-a").await.unwrap();
    assert!(repository.get(&profile.id).await.is_err());
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
        .create(&owner, owner.id(), "A", true, &material("sid=a"))
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

    for replacement in [None, Some(serde_json::json!({"bilibili": {"quality": 2}}))] {
        template.platform_overrides = replacement.as_ref().map(ToString::to_string);
        config.update_template_config(&template).await.unwrap();
        let stored = config.get_template_config(&template.id).await.unwrap();
        assert_eq!(
            overrides(&stored)["bilibili"]["credential_selection"],
            fixed
        );
        assert_eq!(
            overrides(&stored)["bilibili"].get("quality"),
            replacement
                .as_ref()
                .map(|value| &value["bilibili"]["quality"])
        );
    }
    template.platform_overrides = Some(
        serde_json::json!({"bilibili": {"credential_selection": {"mode": "inherit"}}}).to_string(),
    );
    config.update_template_config(&template).await.unwrap();
    let stored = config.get_template_config(&template.id).await.unwrap();
    assert_eq!(
        overrides(&stored)["bilibili"]["credential_selection"],
        serde_json::json!({"mode": "inherit"})
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
        serde_json::json!({"credential_selection": fixed, "quality": 3})
    );
}

#[tokio::test]
async fn a_write_that_strands_a_selection_names_the_referring_configs() {
    use rust_srec::credentials::ProfileError;
    use rust_srec::database::models::{StreamerDbModel, TemplateConfigDbModel};
    use rust_srec::database::repositories::{SqlxStreamerRepository, StreamerRepository};

    let (pool, repository) = fixture().await;
    let config = SqlxConfigRepository::new(pool.clone(), pool.clone());
    let night = TemplateConfigDbModel::new("Night");
    let day = TemplateConfigDbModel::new("Day");
    config.create_template_config(&night).await.unwrap();
    config.create_template_config(&day).await.unwrap();
    let owner = CredentialOwner::Template {
        template_id: night.id.clone(),
    };
    let profile = repository
        .create(
            &owner,
            "platform-bilibili",
            "Night account",
            true,
            &material("sid=a"),
        )
        .await
        .unwrap();
    let streamers = SqlxStreamerRepository::new(pool.clone(), pool.clone());
    let mut streamer = StreamerDbModel::new(
        "Uses the template account",
        "https://live.bilibili.com/2",
        "platform-bilibili",
    );
    streamer.template_config_id = Some(night.id.clone());
    streamer.streamer_specific_config = Some(
        serde_json::json!({"credential_selection": {"mode": "fixed", "credential_id": profile.id}})
            .to_string(),
    );
    streamers.create_streamer(&streamer).await.unwrap();

    streamer.template_config_id = Some(day.id.clone());
    let error = streamers.update_streamer(&streamer).await.unwrap_err();
    assert!(
        matches!(
            &error,
            rust_srec::Error::CredentialProfile(ProfileError::InaccessibleReferences(references))
                if references == &vec![format!("streamer:{}", streamer.id)]
        ),
        "{error}"
    );
    let stored = streamers.get_streamer(&streamer.id).await.unwrap();
    assert_eq!(
        stored.template_config_id.as_deref(),
        Some(night.id.as_str())
    );
}
