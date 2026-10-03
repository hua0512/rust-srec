use super::*;
use crate::config::backup::{CredentialOwnerExport, CredentialProfileExport, PlatformExport};
use crate::credentials::{
    CredentialBinding, CredentialIdentity, CredentialOwner, CredentialSelection,
    ResolvedCredentialPolicy,
};

#[tokio::test]
async fn acceptance_backup_restart_and_concurrent_qr_keep_account_identity() {
    use crate::credentials::login_sessions::{CredentialLoginSessions, CredentialLoginTarget};
    use crate::credentials::{
        CredentialExecutionService, CredentialMaterial, Extracted, OperationDeadline,
        PlatformAdmission, PoolStrategy,
    };
    use crate::database::repositories::{CredentialProfileRepository, SqlxCredentialStore};
    use crate::monitor::{RateLimiterConfig, RateLimiterManager};
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
            let mut account = profile("bilibili", CredentialOwnerExport::Platform { platform_name: "bilibili".into() });
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
        backup.credential_profiles = profiles::export_profiles(&source).await.unwrap();
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
            let admission = Arc::new(PlatformAdmission::new(RateLimiterManager::with_config(RateLimiterConfig {
                max_tokens: 100, initial_tokens: 100, refill_rate: 100.0,
            })));
            let refresh = Arc::new(CredentialRefreshService::new(Arc::new(SqlxCredentialStore::new(database.clone(), database.clone()))).with_admission(admission.clone()));
            let policy = ResolvedCredentialPolicy::new(platform_id.into(), CredentialOwner::Platform { platform_id: platform_id.into() }, selection.clone()).unwrap();
            let execute = |refresh: Arc<CredentialRefreshService>| Arc::new(CredentialExecutionService::new(repository.clone(), refresh));
            let service = execute(refresh.clone());
            let extract = |snapshot: crate::credentials::CredentialSnapshot| async move {
                Ok(Extracted { value: snapshot.material.cookies, session_cookies: None, preserve_health: false })
            };
            for account in ["A", "B", "C"] {
                let result = service.execute(&policy, None, true, OperationDeadline::default(), extract).await.unwrap();
                assert_eq!(result.value, format!("account={account}"));
            }
            let pin = CredentialBinding { identity: CredentialIdentity::Profile { profile_id: ids[1].clone() }, revision: 1, policy: policy.clone(), epoch: 7 };
            // Restart discards cursors, locks and caches, but a persisted binding
            // still requests fresh media for B, even with a new A-first cursor.
            let pin: CredentialBinding = serde_json::from_str(&serde_json::to_string(&pin).unwrap()).unwrap();
            drop(service);
            let restarted = execute(refresh);
            assert_eq!(restarted.execute(&policy, Some(&pin), false, OperationDeadline::default(), extract).await.unwrap().value, "account=B");

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
                    restarted.execute(&policy, Some(&pin), false, OperationDeadline::default(), move |snapshot| {
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
            assert_eq!(restarted.execute(fixed, None, true, OperationDeadline::default(), extract).await.unwrap().value, "account=A");
        }
    }).await.unwrap();
}

fn platform_export(model: &PlatformConfigDbModel) -> PlatformExport {
    let mut value = serde_json::to_value(model).unwrap();
    if let Some(raw) = &model.credential_selection {
        value["credential_selection"] = serde_json::from_str(raw).unwrap();
    }
    serde_json::from_value(value).unwrap()
}

fn profile(platform: &str, owner: CredentialOwnerExport) -> CredentialProfileExport {
    CredentialProfileExport {
        id: uuid::Uuid::new_v4().to_string(),
        platform: platform.into(),
        owner,
        label: "Account".into(),
        enabled: true,
        cookies: "session=secret".into(),
        refresh_token: Some("secret-refresh".into()),
        access_token: None,
        reauth_config: None,
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
async fn all_converted_template_rejects_legacy_scalar_replacement_during_import() {
    let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
    run_migrations(&pool).await.unwrap();
    let mut tx = begin_immediate(&pool).await.unwrap();
    let names: Vec<String> = sqlx::query_scalar("SELECT platform_name FROM platform_config")
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    let overrides: serde_json::Map<String, serde_json::Value> = names
        .into_iter()
        .map(|name| {
            (
                name,
                serde_json::json!({"credential_selection":{"mode":"none"}}),
            )
        })
        .collect();
    sqlx::query("INSERT INTO template_config(id, name, platform_overrides) VALUES ('converted', 'Converted', ?)").bind(serde_json::Value::Object(overrides).to_string()).execute(&mut *tx).await.unwrap();
    let snapshot = ImportSnapshot::load(&mut tx).await.unwrap();
    let mut config = import_config(&snapshot.global);
    config.version = "1.0.0".into();
    let mut template: crate::config::backup::TemplateExport =
        serde_json::from_value(serde_json::to_value(&snapshot.templates["Converted"]).unwrap())
            .unwrap();
    template.cookies = Some("stale=legacy".into());
    config.templates.push(template);
    assert!(matches!(
        apply_import(&mut tx, &snapshot, &config, ImportMode::Merge).await,
        Err(ConfigurationImportError::Conflict(_))
    ));
    let cookies: Option<String> =
        sqlx::query_scalar("SELECT cookies FROM template_config WHERE id = 'converted'")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert!(cookies.is_none());
}

#[tokio::test]
async fn merge_preserves_omitted_policies_but_explicit_inherit_replaces_them() {
    let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
    run_migrations(&pool).await.unwrap();
    let mut tx = begin_immediate(&pool).await.unwrap();
    sqlx::query("UPDATE platform_config SET credential_selection = '{\"mode\":\"none\"}' WHERE id = 'platform-bilibili'").execute(&mut *tx).await.unwrap();
    let snapshot = ImportSnapshot::load(&mut tx).await.unwrap();
    let platform = snapshot.platforms.get("bilibili").unwrap();
    let mut config = import_config(&snapshot.global);
    let mut exported = platform_export(platform);
    exported.credential_selection = None;
    config.platforms.push(exported);
    apply_import(&mut tx, &snapshot, &config, ImportMode::Merge)
        .await
        .unwrap();
    let stored: String = sqlx::query_scalar(
        "SELECT credential_selection FROM platform_config WHERE id = 'platform-bilibili'",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(stored, "{\"mode\":\"none\"}");
    config.version = "1.0.0".into();
    config.platforms[0].credential_selection = Some(CredentialSelection::Inherit);
    let snapshot = ImportSnapshot::load(&mut tx).await.unwrap();
    apply_import(&mut tx, &snapshot, &config, ImportMode::Merge)
        .await
        .unwrap();
    let stored: String = sqlx::query_scalar(
        "SELECT credential_selection FROM platform_config WHERE id = 'platform-bilibili'",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(stored, "{\"mode\":\"inherit\"}");

    let mut template = TemplateConfigDbModel::new("Template");
    template.platform_overrides = Some(
        serde_json::json!({"bilibili":{"credential_selection":{"mode":"none"},"quality":80}})
            .to_string(),
    );
    let mut replacement = template.clone();
    replacement.platform_overrides =
        Some(serde_json::json!({"bilibili":{"quality":120}}).to_string());
    profiles::preserve_template_policies(&template, &mut replacement).unwrap();
    let stored: serde_json::Value =
        serde_json::from_str(replacement.platform_overrides.as_deref().unwrap()).unwrap();
    assert_eq!(stored["bilibili"]["credential_selection"]["mode"], "none");
    assert_eq!(stored["bilibili"]["quality"], 120);
    let mut streamer = StreamerDbModel::new(
        "Streamer",
        "https://example.test/policy",
        "platform-bilibili",
    );
    streamer.streamer_specific_config =
        Some(serde_json::json!({"credential_selection":{"mode":"none"}}).to_string());
    let mut replacement = streamer.clone();
    replacement.streamer_specific_config = None;
    profiles::preserve_streamer_policy(&streamer, &mut replacement).unwrap();
    assert_eq!(
        replacement.streamer_specific_config,
        streamer.streamer_specific_config
    );
}

#[tokio::test]
async fn backup_version_and_filtered_dependency_validation_preserve_legacy_shape() {
    let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
    run_migrations(&pool).await.unwrap();
    let mut tx = begin_immediate(&pool).await.unwrap();
    let snapshot = ImportSnapshot::load(&mut tx).await.unwrap();
    let platform = snapshot.platforms.values().next().unwrap();
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
    let account = profile(
        &platform.platform_name,
        CredentialOwnerExport::Platform {
            platform_name: platform.platform_name.clone(),
        },
    );
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
async fn repeated_profile_import_preserves_uuid_remaps_owner_and_rejects_collisions_atomically() {
    let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
    run_migrations(&pool).await.unwrap();
    let mut tx = begin_immediate(&pool).await.unwrap();
    let snapshot = ImportSnapshot::load(&mut tx).await.unwrap();
    let platform = snapshot.platforms.values().next().unwrap();
    let mut config = import_config(&snapshot.global);
    config.version = "1.0.0".into();
    let url = "https://example.test/import-account";
    let account = profile(
        &platform.platform_name,
        CredentialOwnerExport::Streamer {
            streamer_url: url.into(),
        },
    );
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
        assert_eq!(
            stored.streamer_id.as_deref(),
            Some(snapshot.streamer_for_update(url).unwrap().id.as_str())
        );
        tx.commit().await.unwrap();
    }
    let exports = profiles::export_profiles(&pool).await.unwrap();
    assert_eq!(exports.len(), 1);
    assert!(
        matches!(&exports[0].owner, CredentialOwnerExport::Streamer { streamer_url } if streamer_url == url)
    );
    assert!(!format!("{:?}", exports[0]).contains("secret"));
    let mut tx = begin_immediate(&pool).await.unwrap();
    let snapshot = ImportSnapshot::load(&mut tx).await.unwrap();
    config.global_config.output_folder = "must-rollback".into();
    config.credential_profiles[0].owner = CredentialOwnerExport::Platform {
        platform_name: account.platform.clone(),
    };
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
async fn managed_profile_graph_rejection_and_legacy_import_guard_leave_configuration_unchanged() {
    let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
    run_migrations(&pool).await.unwrap();
    let mut tx = begin_immediate(&pool).await.unwrap();
    let snapshot = ImportSnapshot::load(&mut tx).await.unwrap();
    let platform = snapshot.platforms.values().next().unwrap();
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
    sqlx::query(
        "UPDATE platform_config SET credential_selection = '{\"mode\":\"none\"}' WHERE id = ?",
    )
    .bind(&platform.id)
    .execute(&pool)
    .await
    .unwrap();
    let mut tx = begin_immediate(&pool).await.unwrap();
    let snapshot = ImportSnapshot::load(&mut tx).await.unwrap();
    config.platforms[0].credential_selection = None;
    config.platforms[0].cookies = Some("old=scalar".into());
    config.version = "0.1.8".into();
    assert!(
        apply_import(&mut tx, &snapshot, &config, ImportMode::Merge)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    let policy: String =
        sqlx::query_scalar("SELECT credential_selection FROM platform_config WHERE id = ?")
            .bind(&platform.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(policy, "{\"mode\":\"none\"}");
}

#[tokio::test]
async fn replace_profile_retirement_retains_active_session_material_until_reap() {
    let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
    run_migrations(&pool).await.unwrap();
    let mut tx = begin_immediate(&pool).await.unwrap();
    let snapshot = ImportSnapshot::load(&mut tx).await.unwrap();
    let platform = snapshot.platforms.values().next().unwrap();
    let mut config = import_config(&snapshot.global);
    config.version = "1.0.0".into();
    let account = profile(
        &platform.platform_name,
        CredentialOwnerExport::Platform {
            platform_name: platform.platform_name.clone(),
        },
    );
    config.credential_profiles.push(account.clone());
    let platform_ids = snapshot
        .platforms
        .iter()
        .map(|(name, p)| (name.clone(), p.id.clone()))
        .collect();
    profiles::apply(
        &mut tx,
        &config,
        ImportMode::Merge,
        &HashMap::new(),
        &platform_ids,
    )
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
    profiles::apply(
        &mut tx,
        &config,
        ImportMode::Merge,
        &HashMap::new(),
        &platform_ids,
    )
    .await
    .unwrap();
    let retained = crate::database::repositories::credential_profiles::load(&mut tx, &account.id)
        .await
        .unwrap();
    assert!(retained.enabled);
    profiles::apply(
        &mut tx,
        &config,
        ImportMode::Replace,
        &HashMap::new(),
        &platform_ids,
    )
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
async fn omitted_template_waits_for_profile_binding_after_streamer_reassignment() {
    use crate::database::repositories::config_retirement::{
        RetiredConfigKind, delete_or_defer, reap,
    };
    let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
    run_migrations(&pool).await.unwrap();
    let mut tx = begin_immediate(&pool).await.unwrap();
    let snapshot = ImportSnapshot::load(&mut tx).await.unwrap();
    let platform = snapshot.platforms.values().next().unwrap();
    sqlx::query("INSERT INTO template_config(id, name) VALUES ('old-owner', 'Old owner')")
        .execute(&mut *tx)
        .await
        .unwrap();
    let mut config = import_config(&snapshot.global);
    config.version = "1.0.0".into();
    let account = profile(
        &platform.platform_name,
        CredentialOwnerExport::Template {
            template_name: "Old owner".into(),
        },
    );
    config.credential_profiles.push(account.clone());
    let template_ids = HashMap::from([("Old owner".into(), "old-owner".into())]);
    let platform_ids = snapshot
        .platforms
        .iter()
        .map(|(name, p)| (name.clone(), p.id.clone()))
        .collect();
    profiles::apply(
        &mut tx,
        &config,
        ImportMode::Merge,
        &template_ids,
        &platform_ids,
    )
    .await
    .unwrap();
    let policy = ResolvedCredentialPolicy::new(
        platform.id.clone(),
        CredentialOwner::Template {
            template_id: "old-owner".into(),
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
    sqlx::query("INSERT INTO live_sessions(id, streamer_name, start_time, credential_binding) VALUES ('template-live', 'Test', 1, ?)").bind(serde_json::to_string(&binding).unwrap()).execute(&mut *tx).await.unwrap();
    delete_or_defer(&mut tx, RetiredConfigKind::Template, "old-owner")
        .await
        .unwrap();
    reap(&mut tx).await.unwrap();
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM template_config WHERE id = 'old-owner')")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert!(exists);
    sqlx::query("UPDATE live_sessions SET end_time = 2 WHERE id = 'template-live'")
        .execute(&mut *tx)
        .await
        .unwrap();
    reap(&mut tx).await.unwrap();
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM template_config WHERE id = 'old-owner')")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert!(!exists);
    assert!(
        crate::database::repositories::credential_profiles::load(&mut tx, &account.id)
            .await
            .is_err()
    );
}
