use super::*;
use crate::config::backup::{CredentialProfileExport, PlatformExport};
use crate::credentials::{
    CredentialBinding, CredentialIdentity, CredentialOwner, CredentialSelection,
    ResolvedCredentialPolicy,
};

#[tokio::test]
async fn acceptance_backup_restart_and_concurrent_qr_keep_account_identity() {
    use crate::credentials::login_sessions::{CredentialLoginSessions, CredentialLoginTarget};
    use crate::credentials::{
        CredentialExecutionService, CredentialMaterial, CredentialProviderRegistry, Extracted,
        OperationDeadline, PoolStrategy,
    };
    use crate::database::repositories::CredentialProfileRepository;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    tokio::time::timeout(Duration::from_secs(15), async {
        let source = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
        run_migrations(&source).await.unwrap();
        let mut tx = begin_immediate(&source).await.unwrap();
        let snapshot = ImportSnapshot::load(&mut tx).await.unwrap();
        let mut backup = import_config(&snapshot.global);
        backup.platforms.push(platform_export(&snapshot.platforms["bilibili"]));
        for label in ["A", "B", "C"] {
            let mut account = profile("bilibili");
            account.label = label.into();
            account.cookies = format!("account={label}");
            backup.credential_profiles.push(account);
        }
        let ids: Vec<_> = backup.credential_profiles.iter().map(|profile| profile.id.clone()).collect();
        let selection = CredentialSelection::Pool {
            credential_ids: ids.clone(), strategy: PoolStrategy::RoundRobin, failover: true, max_attempts: 3,
        };
        backup.platforms[0].credential_selection = Some(selection.clone());
        let mut fixed = imported_streamer("https://live.bilibili.com/123", "bilibili");
        fixed.streamer_specific_config = Some(serde_json::json!({"credential_selection":{"mode":"fixed","credential_id":ids[0]}}));
        backup.streamers.push(fixed);
        backup.update_schema_version();
        validate_import(&backup, ImportMode::Merge).unwrap();
        apply_import(&mut tx, &snapshot, &backup, ImportMode::Merge).await.unwrap();
        tx.commit().await.unwrap();
        // The real export reader retains IDs/material; wire serialization must
        // also survive before import maps the platform to a different local ID.
        backup.credential_profiles = profiles::export_profiles(&source, &Default::default()).await.unwrap();
        let serialized = serde_json::to_string(&backup).unwrap();
        let backup: ConfigExport = serde_json::from_str(&serialized).unwrap();
        let restored = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
        run_migrations(&restored).await.unwrap();
        sqlx::query("UPDATE platform_config SET id = 'restored-bilibili' WHERE id = 'platform-bilibili'").execute(&restored).await.unwrap();
        let mut tx = begin_immediate(&restored).await.unwrap();
        let snapshot = ImportSnapshot::load(&mut tx).await.unwrap();
        apply_import(&mut tx, &snapshot, &backup, ImportMode::Merge).await.unwrap();
        tx.commit().await.unwrap();

        for (database, platform_id) in [(&source, "platform-bilibili"), (&restored, "restored-bilibili")] {
            let repository = Arc::new(CredentialProfileRepository::new(database.clone(), database.clone()));
            let admission = crate::credentials::test_support::unthrottled_admission();
            let refresh = Arc::new(CredentialProviderRegistry::new().with_admission(admission.clone()));
            let policy = ResolvedCredentialPolicy::new(platform_id.into(), CredentialOwner::Platform { platform_id: platform_id.into() }, selection.clone()).unwrap();
            let execute = |refresh: Arc<CredentialProviderRegistry>| Arc::new(CredentialExecutionService::new(repository.clone(), refresh));
            let service = execute(refresh.clone());
            let extract = |snapshot: crate::credentials::CredentialSnapshot| async move {
                Ok(Extracted { value: snapshot.material.cookies, session_cookies: None, preserve_health: false })
            };
            for account in ["A", "B", "C"] {
                let result = service.execute(&policy, None, true, OperationDeadline::default(), &crate::proxies::ResolvedRoute::default(), extract).await.unwrap();
                assert_eq!(result.value, format!("account={account}"));
            }
            let pin = CredentialBinding { identity: CredentialIdentity::Profile { profile_id: ids[1].clone() }, revision: 1, policy: policy.clone(), epoch: 7 };
            // Restart discards cursors, locks and caches, but a persisted binding
            // still requests fresh media for B, even with a new A-first cursor.
            let pin: CredentialBinding = serde_json::from_str(&serde_json::to_string(&pin).unwrap()).unwrap();
            drop(service);
            let restarted = execute(refresh);
            assert_eq!(restarted.execute(&policy, Some(&pin), false, OperationDeadline::default(), &crate::proxies::ResolvedRoute::default(), extract).await.unwrap().value, "account=B");

            let login = CredentialLoginSessions::new(repository.clone(), database.clone(), database.clone(), admission);
            let target = CredentialLoginTarget::Replace { profile_id: ids[1].clone(), expected_version: 1 };
            let now = crate::database::time::now_ms();
            sqlx::query("INSERT INTO credential_login_sessions(id, principal, platform_config_id, target, provider_auth_code, created_at, expires_at) VALUES ('qr-race', 'operator', ?, ?, 'fake-provider-receipt', ?, ?)")
                .bind(platform_id).bind(serde_json::to_string(&target).unwrap()).bind(now).bind(now + 300_000).execute(database).await.unwrap();
            let entered = Arc::new(tokio::sync::Notify::new());
            let release = Arc::new(tokio::sync::Semaphore::new(0));
            let calls = Arc::new(AtomicUsize::new(0));
            let operation = {
                let restarted = restarted.clone(); let policy = policy.clone(); let pin = pin.clone();
                let entered = entered.clone(); let release = release.clone(); let calls = calls.clone();
                tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
                    restarted.execute(&policy, Some(&pin), false, OperationDeadline::default(), &crate::proxies::ResolvedRoute::default(), move |snapshot| {
                        let first = calls.fetch_add(1, Ordering::SeqCst) == 0;
                        let entered = entered.clone(); let release = release.clone();
                        async move {
                            if first { entered.notify_one(); release.acquire().await.unwrap().forget(); }
                            Ok(Extracted {
                                value: snapshot.material.cookies,
                                session_cookies: first.then(|| "stale=provider-result".into()), preserve_health: false,
                            })
                        }
                    }).await.unwrap()
                }))
            };
            entered.notified().await;
            let material = CredentialMaterial { cookies: "account=B-new-login".into(), refresh_token: None, access_token: None, reauth_config: None };
            // Provider success is faked; concurrent local receipts use the
            // production completion transaction and can replace B only once.
            let (one, two) = tokio::join!(login.complete("operator", "qr-race", &material), login.complete("operator", "qr-race", &material));
            let (one, two) = (one.unwrap(), two.unwrap());
            assert_eq!(one.status, "completed");
            assert_eq!(one.profile_id.as_deref(), Some(ids[1].as_str()));
            assert_eq!(one.version, two.version);
            release.add_permits(1);
            let fresh = operation.await.unwrap();
            assert_eq!(fresh.value, "account=B-new-login");
            assert_eq!(fresh.snapshot.binding.identity, pin.identity);
            assert_eq!(fresh.snapshot.binding.revision, 2);
            assert_eq!(calls.load(Ordering::SeqCst), 2);
            assert_eq!(repository.get(&ids[1]).await.unwrap().cookies, "account=B-new-login");
            for index in [0, 2] {
                let unchanged = repository.get(&ids[index]).await.unwrap();
                assert_eq!(unchanged.cookies, if index == 0 { "account=A" } else { "account=C" });
                assert_eq!(unchanged.revision, 1);
            }
            let config_service = ConfigService::new(
                Arc::new(SqlxConfigRepository::new(database.clone(), database.clone())),
                Arc::new(SqlxStreamerRepository::new(database.clone(), database.clone())),
            );
            let streamer_id: String = sqlx::query_scalar("SELECT id FROM streamers WHERE url = 'https://live.bilibili.com/123'").fetch_one(database).await.unwrap();
            let fixed = config_service.get_config_for_streamer(&streamer_id).await.unwrap();
            let fixed = fixed.credential_policy.as_ref().unwrap();
            assert_eq!(restarted.execute(fixed, None, true, OperationDeadline::default(), &crate::proxies::ResolvedRoute::default(), extract).await.unwrap().value, "account=A");
        }
    }).await.unwrap();
}

fn platform_export(model: &PlatformConfigDbModel) -> PlatformExport {
    serde_json::from_value(serde_json::to_value(model).unwrap()).unwrap()
}

fn template_export(
    name: &str,
    overrides: serde_json::Value,
) -> crate::config::backup::TemplateExport {
    serde_json::from_value(serde_json::json!({"name": name, "platform_overrides": overrides}))
        .unwrap()
}

async fn stored(
    connection: &mut sqlx::SqliteConnection,
    owner: CredentialOwner,
    platform_id: &str,
) -> Option<CredentialSelection> {
    credential_selections::test_support::load_scope(connection, &owner, platform_id)
        .await
        .unwrap()
}

fn bilibili() -> CredentialOwner {
    CredentialOwner::Platform {
        platform_id: "platform-bilibili".into(),
    }
}

fn profile(platform: &str) -> CredentialProfileExport {
    CredentialProfileExport {
        id: uuid::Uuid::new_v4().to_string(),
        platform: platform.into(),
        label: "Account".into(),
        enabled: true,
        cookies: "session=secret".into(),
        refresh_token: Some("secret-refresh".into()),
        access_token: None,
        reauth_config: None,
        proxy_route: None,
    }
}

#[test]
fn legacy_versions_cannot_hide_managed_policies_in_double_encoded_documents() {
    let mut config = import_config(&GlobalConfigDbModel::default());
    let mut streamer = imported_streamer("https://example.test/hidden-policy", "bilibili");
    streamer.streamer_specific_config = Some(serde_json::json!(
        r#"{"credential_selection":{"mode":"inherit"}}"#
    ));
    config.streamers.push(streamer);
    assert!(profiles::validate(&config).is_err());
    config.version = "1.0.0".into();
    profiles::validate(&config).unwrap();
    config.streamers[0].streamer_specific_config =
        Some(serde_json::json!({"credential_selection":null}));
    assert!(profiles::validate(&config).is_err());
    config.streamers.clear();
    config.version = "1.1.0".into();
    assert!(validate_import(&config, ImportMode::Merge).is_err());
}

#[tokio::test]
async fn merge_preserves_omitted_policies_but_explicit_inherit_replaces_them() {
    let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
    run_migrations(&pool).await.unwrap();
    let mut tx = begin_immediate(&pool).await.unwrap();
    credential_selections::set(
        &mut tx,
        &bilibili(),
        "platform-bilibili",
        &CredentialSelection::None,
    )
    .await
    .unwrap();
    let snapshot = ImportSnapshot::load(&mut tx).await.unwrap();
    let platform = snapshot.platforms.get("bilibili").unwrap();
    let mut config = import_config(&snapshot.global);
    config.version = "1.0.0".into();
    let mut exported = platform_export(platform);
    exported.credential_selection = None;
    config.platforms.push(exported);
    config.templates.push(template_export(
        "Template",
        serde_json::json!({"bilibili":{"credential_selection":{"mode":"none"},"quality":80}}),
    ));
    let mut streamer = imported_streamer("https://live.bilibili.com/77", "bilibili");
    streamer.streamer_specific_config =
        Some(serde_json::json!({"credential_selection":{"mode":"none"}}));
    config.streamers.push(streamer);
    apply_import(&mut tx, &snapshot, &config, ImportMode::Merge)
        .await
        .unwrap();
    assert_eq!(
        stored(&mut tx, bilibili(), "platform-bilibili").await,
        Some(CredentialSelection::None)
    );

    // Omitted selections stay; the stored documents never carry one.
    config.templates[0].platform_overrides = Some(serde_json::json!({"bilibili":{"quality":120}}));
    config.streamers[0].streamer_specific_config = None;
    let snapshot = ImportSnapshot::load(&mut tx).await.unwrap();
    apply_import(&mut tx, &snapshot, &config, ImportMode::Merge)
        .await
        .unwrap();
    let template = &ImportSnapshot::load(&mut tx).await.unwrap().templates["Template"];
    let overrides: serde_json::Value =
        serde_json::from_str(template.platform_overrides.as_deref().unwrap()).unwrap();
    assert_eq!(overrides, serde_json::json!({"bilibili":{"quality":120}}));
    let template = CredentialOwner::Template {
        template_id: template.id.clone(),
    };
    assert_eq!(
        stored(&mut tx, template.clone(), "platform-bilibili").await,
        Some(CredentialSelection::None)
    );
    let streamer_id: String =
        sqlx::query_scalar("SELECT id FROM streamers WHERE url = 'https://live.bilibili.com/77'")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    let streamer = CredentialOwner::Streamer { streamer_id };
    assert_eq!(
        stored(&mut tx, streamer.clone(), "platform-bilibili").await,
        Some(CredentialSelection::None)
    );

    // An explicit inherit removes the stored selection.
    config.platforms[0].credential_selection = Some(CredentialSelection::Inherit);
    let snapshot = ImportSnapshot::load(&mut tx).await.unwrap();
    apply_import(&mut tx, &snapshot, &config, ImportMode::Merge)
        .await
        .unwrap();
    assert_eq!(stored(&mut tx, bilibili(), "platform-bilibili").await, None);

    // Replace takes the bundle as written: omitted selections are removed.
    let snapshot = ImportSnapshot::load(&mut tx).await.unwrap();
    apply_import(&mut tx, &snapshot, &config, ImportMode::Replace)
        .await
        .unwrap();
    assert_eq!(stored(&mut tx, template, "platform-bilibili").await, None);
    assert_eq!(stored(&mut tx, streamer, "platform-bilibili").await, None);
}

#[tokio::test]
async fn backup_version_and_filtered_dependency_validation_preserve_legacy_shape() {
    let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
    run_migrations(&pool).await.unwrap();
    let mut tx = begin_immediate(&pool).await.unwrap();
    let snapshot = ImportSnapshot::load(&mut tx).await.unwrap();
    let platform = cookie_platform(&snapshot);
    let mut config = import_config(&snapshot.global);
    config.platforms.push(platform_export(platform));
    config.update_schema_version();
    assert_eq!(config.version, "0.1.8");
    let value = serde_json::to_value(&config).unwrap();
    assert!(value.get("credential_profiles").is_none());
    assert!(value["platforms"][0].get("credential_selection").is_none());
    for selection in [CredentialSelection::Inherit, CredentialSelection::None] {
        config.platforms[0].credential_selection = Some(selection);
        config.update_schema_version();
        assert_eq!(config.version, "1.0.0");
    }
    let account = profile(&platform.platform_name);
    config.platforms[0].credential_selection = Some(CredentialSelection::Fixed {
        credential_id: account.id.clone(),
    });
    assert!(config.validate_credential_graph().is_err());
    config.credential_profiles.push(account);
    config.validate_credential_graph().unwrap();
    config.platforms.clear();
    assert!(config.validate_credential_graph().is_err());
    config.credential_profiles.clear();
    config.update_schema_version();
    assert_eq!(config.version, "0.1.8");
}

#[tokio::test]
async fn repeated_profile_import_preserves_uuid_and_rejects_collisions_atomically() {
    let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
    run_migrations(&pool).await.unwrap();
    let mut tx = begin_immediate(&pool).await.unwrap();
    let snapshot = ImportSnapshot::load(&mut tx).await.unwrap();
    let platform = cookie_platform(&snapshot);
    let mut config = import_config(&snapshot.global);
    config.version = "1.0.0".into();
    let url = "https://example.test/import-account";
    let account = profile(&platform.platform_name);
    let mut streamer = imported_streamer(url, &platform.platform_name);
    streamer.streamer_specific_config = Some(
        serde_json::json!({"credential_selection":{"mode":"fixed", "credential_id":account.id}}),
    );
    config.streamers.push(streamer);
    config.credential_profiles.push(account.clone());
    validate_import(&config, ImportMode::Merge).unwrap();
    apply_import(&mut tx, &snapshot, &config, ImportMode::Merge)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    for expected_revision in [2_i64, 3] {
        if expected_revision == 3 {
            config.credential_profiles[0].refresh_token = None;
            config.proxies.push(crate::config::backup::ProxyExport {
                name: "Account proxy".into(),
                url: "http://proxy.example:8080".into(),
                username: Some("user".into()),
                password: Some("secret-proxy".into()),
            });
            config.credential_profiles[0].proxy_route =
                Some(crate::config::backup::BackupRoute::Proxy {
                    name: "account proxy".into(),
                });
        }
        let mut tx = begin_immediate(&pool).await.unwrap();
        let snapshot = ImportSnapshot::load(&mut tx).await.unwrap();
        apply_import(&mut tx, &snapshot, &config, ImportMode::Merge)
            .await
            .unwrap();
        let stored = crate::database::repositories::credential_profiles::load(&mut tx, &account.id)
            .await
            .unwrap();
        assert_eq!(stored.revision, expected_revision);
        assert_eq!(
            stored.refresh_token,
            config.credential_profiles[0].refresh_token
        );
        if expected_revision == 3 {
            let entry =
                crate::database::repositories::proxies::find_by_name(&mut tx, "Account proxy")
                    .await
                    .unwrap()
                    .unwrap();
            assert_eq!(entry.password.as_deref(), Some("secret-proxy"));
            assert_eq!(
                stored.route().unwrap(),
                crate::proxies::ProxyRoute::Proxy { id: entry.id }
            );
        } else {
            assert_eq!(stored.route().unwrap(), crate::proxies::ProxyRoute::Inherit);
        }
        tx.commit().await.unwrap();
    }
    let names: std::collections::HashMap<String, String> =
        crate::database::repositories::proxies::list(&mut pool.acquire().await.unwrap())
            .await
            .unwrap()
            .into_iter()
            .map(|entry| (entry.id, entry.name))
            .collect();
    let exports = profiles::export_profiles(&pool, &names).await.unwrap();
    assert_eq!(exports.len(), 1);
    assert_eq!(exports[0].platform, account.platform);
    assert_eq!(
        exports[0].proxy_route,
        Some(crate::config::backup::BackupRoute::Proxy {
            name: "Account proxy".into()
        })
    );
    assert!(!format!("{:?}", exports[0]).contains("secret"));
    let mut unroutable = config.clone();
    unroutable.proxies[0].url = "ftp://proxy.example".into();
    assert!(validate_import(&unroutable, ImportMode::Merge).is_err());
    let mut tx = begin_immediate(&pool).await.unwrap();
    let snapshot = ImportSnapshot::load(&mut tx).await.unwrap();
    config.global_config.output_folder = "must-rollback".into();
    // The UUID already belongs to another platform's profile.
    config.credential_profiles[0].platform = snapshot
        .platforms
        .keys()
        .find(|name| **name != account.platform)
        .unwrap()
        .clone();
    assert!(
        apply_import(&mut tx, &snapshot, &config, ImportMode::Merge)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    let output: String = sqlx::query_scalar("SELECT output_folder FROM global_config LIMIT 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_ne!(output, "must-rollback");
}

#[tokio::test]
async fn managed_profile_graph_and_configuration_credentials_are_rejected_without_changes() {
    let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
    run_migrations(&pool).await.unwrap();
    let mut tx = begin_immediate(&pool).await.unwrap();
    let snapshot = ImportSnapshot::load(&mut tx).await.unwrap();
    let platform = cookie_platform(&snapshot);
    let mut config = import_config(&snapshot.global);
    config.platforms.push(platform_export(platform));
    config.platforms[0].credential_selection = Some(CredentialSelection::Fixed {
        credential_id: uuid::Uuid::new_v4().to_string(),
    });
    config.version = "1.0.0".into();
    assert!(
        apply_import(&mut tx, &snapshot, &config, ImportMode::Merge)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    credential_selections::test_support::set_platform_selection(
        &pool,
        &platform.id,
        &CredentialSelection::None,
    )
    .await;
    let mut tx = begin_immediate(&pool).await.unwrap();
    let snapshot = ImportSnapshot::load(&mut tx).await.unwrap();
    // Account fields belong in profiles; configuration carrying them is rejected.
    config.platforms[0].credential_selection = None;
    config.platforms[0].platform_specific_config =
        Some(serde_json::json!({"refresh_token":"stale-token"}));
    config.version = "0.1.8".into();
    assert!(
        apply_import(&mut tx, &snapshot, &config, ImportMode::Merge)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    let mut connection = pool.acquire().await.unwrap();
    let owner = CredentialOwner::Platform {
        platform_id: platform.id.clone(),
    };
    assert_eq!(
        stored(&mut connection, owner, &platform.id).await,
        Some(CredentialSelection::None)
    );
}

#[tokio::test]
async fn replace_profile_retirement_retains_active_session_material_until_reap() {
    let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
    run_migrations(&pool).await.unwrap();
    let mut tx = begin_immediate(&pool).await.unwrap();
    let snapshot = ImportSnapshot::load(&mut tx).await.unwrap();
    let platform = cookie_platform(&snapshot);
    let mut config = import_config(&snapshot.global);
    config.version = "1.0.0".into();
    let account = profile(&platform.platform_name);
    config.credential_profiles.push(account.clone());
    let platform_ids = snapshot
        .platforms
        .iter()
        .map(|(name, p)| (name.clone(), p.id.clone()))
        .collect();
    profiles::upsert(&mut tx, &config, &platform_ids, &Default::default(), false)
        .await
        .unwrap();
    let policy = ResolvedCredentialPolicy::new(
        platform.id.clone(),
        CredentialOwner::Platform {
            platform_id: platform.id.clone(),
        },
        CredentialSelection::Fixed {
            credential_id: account.id.clone(),
        },
    )
    .unwrap();
    let binding = CredentialBinding {
        identity: CredentialIdentity::Profile {
            profile_id: account.id.clone(),
        },
        revision: 1,
        policy,
        epoch: 1,
    };
    sqlx::query("INSERT INTO live_sessions(id, streamer_name, start_time, credential_binding) VALUES ('profile-live', 'Test', 1, ?)").bind(serde_json::to_string(&binding).unwrap()).execute(&mut *tx).await.unwrap();
    config.credential_profiles.clear();
    profiles::retire_omitted(&mut tx, &config, ImportMode::Merge)
        .await
        .unwrap();
    let retained = crate::database::repositories::credential_profiles::load(&mut tx, &account.id)
        .await
        .unwrap();
    assert!(retained.enabled);
    profiles::retire_omitted(&mut tx, &config, ImportMode::Replace)
        .await
        .unwrap();
    let retained = crate::database::repositories::credential_profiles::load(&mut tx, &account.id)
        .await
        .unwrap();
    assert!(!retained.enabled);
    assert_eq!(retained.cookies, "session=secret");
    sqlx::query("UPDATE live_sessions SET end_time = 2 WHERE id = 'profile-live'")
        .execute(&mut *tx)
        .await
        .unwrap();
    crate::database::repositories::config_retirement::reap_retired_profiles(&mut tx)
        .await
        .unwrap();
    assert!(
        crate::database::repositories::credential_profiles::load(&mut tx, &account.id)
            .await
            .is_err()
    );
    let violations = sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    assert!(violations.is_empty());
}

#[tokio::test]
async fn backups_from_before_profiles_convert_their_cookies_on_import() {
    let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
    run_migrations(&pool).await.unwrap();
    let mut tx = begin_immediate(&pool).await.unwrap();
    let snapshot = ImportSnapshot::load(&mut tx).await.unwrap();
    let mut config = import_config(&snapshot.global);
    let mut platform = platform_export(&snapshot.platforms["bilibili"]);
    platform.cookies = Some("SESSDATA=backup".into());
    platform.platform_specific_config =
        Some(serde_json::json!({"refresh_token":"backup-refresh","quality":"origin"}));
    config.platforms.push(platform);
    for url in [
        "https://live.bilibili.com/91",
        "https://live.bilibili.com/92",
    ] {
        let mut streamer = imported_streamer(url, "bilibili");
        streamer.streamer_specific_config = Some(serde_json::json!({"cookies":"SESSDATA=local"}));
        config.streamers.push(streamer);
    }
    validate_import(&config, ImportMode::Merge).unwrap();
    let version = config.version.clone();

    snapshot
        .upgrade_bundle(&mut config, ImportMode::Merge)
        .unwrap();
    assert_eq!(config.version, version);
    // The platform account, and one account shared by both streamers.
    assert_eq!(config.credential_profiles.len(), 2);
    apply_import(&mut tx, &snapshot, &config, ImportMode::Merge)
        .await
        .unwrap();

    let fields: String = sqlx::query_scalar(
        "SELECT platform_specific_config FROM platform_config WHERE id = 'platform-bilibili'",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&fields).unwrap(),
        serde_json::json!({"quality":"origin"})
    );
    let Some(CredentialSelection::Fixed { credential_id }) =
        stored(&mut tx, bilibili(), "platform-bilibili").await
    else {
        panic!("the platform selects its converted account");
    };
    let account = crate::database::repositories::credential_profiles::load(&mut tx, &credential_id)
        .await
        .unwrap();
    assert_eq!(account.cookies, "SESSDATA=backup");
    assert_eq!(account.refresh_token.as_deref(), Some("backup-refresh"));
    let streamers: Vec<(String, String)> = sqlx::query_as(
        "SELECT id, streamer_specific_config FROM streamers WHERE url LIKE 'https://live.bilibili.com/9%' ORDER BY url",
    )
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    assert_eq!(streamers.len(), 2);
    let mut selected = Vec::new();
    for (streamer_id, config) in streamers {
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&config).unwrap(),
            serde_json::json!({})
        );
        selected.push(
            stored(
                &mut tx,
                CredentialOwner::Streamer { streamer_id },
                "platform-bilibili",
            )
            .await,
        );
    }
    assert_eq!(selected[0], selected[1]);
    let Some(CredentialSelection::Fixed { credential_id }) = &selected[0] else {
        panic!("both streamers select the shared converted account");
    };
    let local = crate::database::repositories::credential_profiles::load(&mut tx, credential_id)
        .await
        .unwrap();
    assert_eq!(local.cookies, "SESSDATA=local");
}

async fn streamer_selection(
    connection: &mut sqlx::SqliteConnection,
    url: &str,
) -> Option<CredentialSelection> {
    let streamer_id: String = sqlx::query_scalar("SELECT id FROM streamers WHERE url = ?")
        .bind(url)
        .fetch_one(&mut *connection)
        .await
        .unwrap();
    stored(
        connection,
        CredentialOwner::Streamer { streamer_id },
        "platform-streamlink",
    )
    .await
}

#[tokio::test]
async fn old_backups_turn_a_streamlink_platform_cookie_into_per_streamer_accounts() {
    let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
    run_migrations(&pool).await.unwrap();
    let mut tx = begin_immediate(&pool).await.unwrap();
    let snapshot = ImportSnapshot::load(&mut tx).await.unwrap();
    let mut config = import_config(&snapshot.global);
    let mut platform = platform_export(&snapshot.platforms["streamlink"]);
    platform.cookies = Some("site=platform".into());
    config.platforms.push(platform);
    for url in ["https://example.test/a", "https://example.test/b"] {
        config.streamers.push(imported_streamer(url, "streamlink"));
    }
    let mut own = imported_streamer("https://example.test/own", "streamlink");
    own.streamer_specific_config = Some(serde_json::json!({"cookies":"site=own"}));
    config.streamers.push(own);
    validate_import(&config, ImportMode::Merge).unwrap();

    snapshot
        .upgrade_bundle(&mut config, ImportMode::Merge)
        .unwrap();
    assert_eq!(config.credential_profiles.len(), 2);
    assert!(config.platforms[0].credential_selection.is_none());
    apply_import(&mut tx, &snapshot, &config, ImportMode::Merge)
        .await
        .unwrap();

    let streamlink = CredentialOwner::Platform {
        platform_id: "platform-streamlink".into(),
    };
    assert_eq!(
        stored(&mut tx, streamlink, "platform-streamlink").await,
        None
    );
    let a = streamer_selection(&mut tx, "https://example.test/a").await;
    assert_eq!(
        a,
        streamer_selection(&mut tx, "https://example.test/b").await
    );
    let Some(CredentialSelection::Fixed { credential_id }) = a else {
        panic!("each streamer selects the platform's converted account");
    };
    let account = crate::database::repositories::credential_profiles::load(&mut tx, &credential_id)
        .await
        .unwrap();
    assert_eq!(
        (account.label.as_str(), account.cookies.as_str()),
        ("streamlink (migrated)", "site=platform")
    );
    let Some(CredentialSelection::Fixed { credential_id }) =
        streamer_selection(&mut tx, "https://example.test/own").await
    else {
        panic!("a streamer with its own cookie keeps it");
    };
    let account = crate::database::repositories::credential_profiles::load(&mut tx, &credential_id)
        .await
        .unwrap();
    assert_eq!(account.cookies, "site=own");
}

#[tokio::test]
async fn streamlink_platform_and_template_selections_in_backups_move_to_streamers() {
    let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
    run_migrations(&pool).await.unwrap();
    let mut tx = begin_immediate(&pool).await.unwrap();
    let snapshot = ImportSnapshot::load(&mut tx).await.unwrap();
    let mut config = import_config(&snapshot.global);
    config.version = "1.0.0".into();
    let accounts: Vec<CredentialProfileExport> = (0..3).map(|_| profile("streamlink")).collect();
    let id = |index: usize| accounts[index].id.clone();
    config.credential_profiles.extend(accounts.iter().cloned());
    let mut platform = platform_export(&snapshot.platforms["streamlink"]);
    platform.credential_selection = Some(CredentialSelection::Fixed {
        credential_id: id(0),
    });
    config.platforms.push(platform);
    config.templates.push(template_export(
        "Pooled",
        serde_json::json!({"streamlink":{"credential_selection":{"mode":"pool","credential_ids":[id(1), id(2)]},"quality":"best"}}),
    ));
    config.templates.push(template_export(
        "Anonymous",
        serde_json::json!({"streamlink":{"credential_selection":{"mode":"none"}}}),
    ));
    let inherits = imported_streamer("https://example.test/platform", "streamlink");
    let mut pooled = imported_streamer("https://example.test/pooled", "streamlink");
    pooled.template = Some("Pooled".into());
    let mut anonymous = imported_streamer("https://example.test/anonymous", "streamlink");
    anonymous.template = Some("Anonymous".into());
    let mut own_pool = imported_streamer("https://example.test/own-pool", "streamlink");
    own_pool.streamer_specific_config = Some(serde_json::json!({"credential_selection":{
        "mode":"pool","credential_ids":[id(2), id(0)],"strategy":"round_robin"
    }}));
    config
        .streamers
        .extend([inherits, pooled, anonymous, own_pool]);
    validate_import(&config, ImportMode::Merge).unwrap();

    snapshot
        .upgrade_bundle(&mut config, ImportMode::Merge)
        .unwrap();
    assert!(config.platforms[0].credential_selection.is_none());
    assert_eq!(
        config.templates[0].platform_overrides,
        Some(serde_json::json!({"streamlink":{"quality":"best"}}))
    );
    apply_import(&mut tx, &snapshot, &config, ImportMode::Merge)
        .await
        .unwrap();

    let fixed = |index: usize| {
        Some(CredentialSelection::Fixed {
            credential_id: id(index),
        })
    };
    assert_eq!(
        streamer_selection(&mut tx, "https://example.test/platform").await,
        fixed(0)
    );
    assert_eq!(
        streamer_selection(&mut tx, "https://example.test/pooled").await,
        fixed(1)
    );
    assert_eq!(
        streamer_selection(&mut tx, "https://example.test/anonymous").await,
        None
    );
    assert_eq!(
        streamer_selection(&mut tx, "https://example.test/own-pool").await,
        fixed(2)
    );
    let remaining: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM credential_selections WHERE platform_config_id = 'platform-streamlink' AND streamer_id IS NULL",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(remaining, 0);
}

#[tokio::test]
async fn merge_import_keeps_stored_streamlink_selections_the_bundle_omits() {
    let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
    run_migrations(&pool).await.unwrap();
    let mut tx = begin_immediate(&pool).await.unwrap();
    let snapshot = ImportSnapshot::load(&mut tx).await.unwrap();
    let stored_account = profile("streamlink");
    let platform_account = profile("streamlink");
    let own = serde_json::json!({"credential_selection":{"mode":"fixed","credential_id":stored_account.id}});
    let mut config = import_config(&snapshot.global);
    config.version = "1.0.0".into();
    config
        .credential_profiles
        .extend([stored_account.clone(), platform_account.clone()]);
    config
        .platforms
        .push(platform_export(&snapshot.platforms["streamlink"]));
    for url in ["https://example.test/kept", "https://example.test/cleared"] {
        let mut streamer = imported_streamer(url, "streamlink");
        streamer.streamer_specific_config = Some(own.clone());
        config.streamers.push(streamer);
    }
    apply_import(&mut tx, &snapshot, &config, ImportMode::Merge)
        .await
        .unwrap();

    // The platform selection moves only onto streamers with no selection of
    // their own, stored or in the bundle; an explicit inherit clears one.
    config.platforms[0].credential_selection = Some(CredentialSelection::Fixed {
        credential_id: platform_account.id.clone(),
    });
    config.streamers[0].streamer_specific_config = None;
    config.streamers[1].streamer_specific_config =
        Some(serde_json::json!({"credential_selection":{"mode":"inherit"}}));
    config
        .streamers
        .push(imported_streamer("https://example.test/new", "streamlink"));
    let snapshot = ImportSnapshot::load(&mut tx).await.unwrap();
    snapshot
        .upgrade_bundle(&mut config, ImportMode::Merge)
        .unwrap();
    apply_import(&mut tx, &snapshot, &config, ImportMode::Merge)
        .await
        .unwrap();
    let fixed = |account: &CredentialProfileExport| {
        Some(CredentialSelection::Fixed {
            credential_id: account.id.clone(),
        })
    };
    assert_eq!(
        streamer_selection(&mut tx, "https://example.test/kept").await,
        fixed(&stored_account)
    );
    assert_eq!(
        streamer_selection(&mut tx, "https://example.test/cleared").await,
        fixed(&platform_account)
    );
    assert_eq!(
        streamer_selection(&mut tx, "https://example.test/new").await,
        fixed(&platform_account)
    );

    // A cookie from before profiles is localized the same way.
    let mut legacy = import_config(&snapshot.global);
    let mut platform = platform_export(&snapshot.platforms["streamlink"]);
    platform.cookies = Some("site=legacy".into());
    legacy.platforms.push(platform);
    legacy
        .streamers
        .push(imported_streamer("https://example.test/kept", "streamlink"));
    let snapshot = ImportSnapshot::load(&mut tx).await.unwrap();
    snapshot
        .upgrade_bundle(&mut legacy, ImportMode::Merge)
        .unwrap();
    apply_import(&mut tx, &snapshot, &legacy, ImportMode::Merge)
        .await
        .unwrap();
    assert_eq!(
        streamer_selection(&mut tx, "https://example.test/kept").await,
        fixed(&stored_account)
    );
}

/// A platform with plain cookie accounts and no per-streamer selection rules,
/// so the outcome does not depend on which platform map order yields first.
fn cookie_platform(snapshot: &ImportSnapshot) -> &crate::database::models::PlatformConfigDbModel {
    snapshot
        .platforms
        .values()
        .find(|platform| platform.platform_name.eq_ignore_ascii_case("huya"))
        .unwrap()
}
