//! Account selections written through the HTTP API and backups reach the next
//! resolution, and backups carry them inside the configuration they belong to.

use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::{FromRef, Path, State};
use axum::response::IntoResponse;
use serde_json::{Value, json};

use super::ServiceContainer;
use crate::api::routes::{config, export_import, streamers, templates};
use crate::api::server::{ApiServices, AppState};
use crate::config::backup::{ConfigExport, ImportMode};
use crate::credentials::{CredentialMaterial, CredentialOwner, CredentialSelection};

struct Fixture {
    _directory: tempfile::TempDir,
    container: Arc<ServiceContainer>,
    state: AppState,
}

async fn fixture() -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    crate::database::run_migrations(&pool).await.unwrap();
    let container = Arc::new(ServiceContainer::new(pool.clone(), pool).await.unwrap());
    let (logging, _layer) =
        crate::logging::LoggingConfig::for_route_tests(directory.path().to_owned());
    let state = AppState::new(ApiServices {
        config_service: container.config_service.clone(),
        streamer_manager: container.streamer_manager.clone(),
        pipeline_manager: container.pipeline_manager.clone(),
        download_manager: container.download_manager.clone(),
        session_repository: container.session_repository.clone(),
        session_event_repository: container.session_event_repository.clone(),
        streamer_check_history_repository: container.streamer_check_history_repository.clone(),
        check_history_broadcaster: container.check_history_broadcaster.clone(),
        upload_status_broadcaster: container.upload_status_broadcaster.clone(),
        upload_record_repository: container.upload_record_repository.clone(),
        filter_repository: container.filter_repository.clone(),
        health_checker: container.health_checker.clone(),
        streamer_repository: container.streamer_repository.clone(),
        pipeline_preset_repository: container.pipeline_preset_repository.clone(),
        job_preset_repository: container.job_preset_repository.clone(),
        notification_repository: container.notification_repository.clone(),
        notification_service: container.notification_service.clone(),
        logging_config: Arc::new(logging),
        logging_download_tokens: Arc::new(dashmap::DashMap::new()),
        logging_archives: Arc::new(crate::api::routes::logging::LogArchiveService::new()),
        credential_service: container.credential_service.clone(),
        platform_admission: container.platform_admission.clone(),
        credential_profiles: container.credential_profiles.clone(),
        credential_execution: container.credential_execution.clone(),
        proxies: container.proxies.clone(),
        credential_blocks: container.stream_monitor.credential_blocks().clone(),
        playback_contexts: container.playback_contexts.clone(),
        credential_login_sessions: container.credential_login_sessions.clone(),
        configuration_import_service: container.configuration_import_service.clone(),
        runtime_coordinator: container.runtime_coordinator.clone(),
    });
    Fixture {
        _directory: directory,
        container,
        state,
    }
}

impl Fixture {
    async fn profile(&self, platform_id: &str, cookies: &str) -> String {
        self.state
            .credential_profiles
            .create(
                platform_id,
                cookies,
                true,
                &CredentialMaterial {
                    cookies: cookies.into(),
                    refresh_token: None,
                    access_token: None,
                    reauth_config: None,
                },
                &crate::proxies::ProxyRoute::Inherit,
            )
            .await
            .unwrap()
            .id
    }

    /// The owner and selection the next resolution picks for a streamer.
    async fn resolved(&self, streamer_id: &str) -> Option<(CredentialOwner, CredentialSelection)> {
        self.container
            .config_service
            .get_config_for_streamer(streamer_id)
            .await
            .unwrap()
            .credential_policy
            .clone()
            .map(|policy| (policy.owner, policy.selection))
    }

    async fn update_streamer(&self, id: &str, request: Value) -> Value {
        let Json(response) = streamers::update_streamer(
            State(streamers::StreamerRouteState::from_ref(&self.state)),
            Path(id.to_owned()),
            Json(serde_json::from_value(request).unwrap()),
        )
        .await
        .unwrap();
        serde_json::to_value(response).unwrap()
    }

    async fn update_template(&self, id: &str, request: Value) -> Value {
        let Json(response) = templates::update_template(
            State(templates::TemplateRouteState::from_ref(&self.state)),
            Path(id.to_owned()),
            Json(serde_json::from_value(request).unwrap()),
        )
        .await
        .unwrap();
        serde_json::to_value(response).unwrap()
    }

    async fn replace_platform(&self, id: &str, selection: Option<Value>) -> Value {
        let state = config::ConfigRouteState::from_ref(&self.state);
        let Json(current) = config::get_platform_config(State(state.clone()), Path(id.to_owned()))
            .await
            .unwrap();
        let mut request = current;
        request.credential_selection = selection.map(|selection| selection.to_string());
        let Json(response) =
            config::replace_platform_config(State(state), Path(id.to_owned()), Json(request))
                .await
                .unwrap();
        serde_json::to_value(response).unwrap()
    }

    async fn export(&self) -> ConfigExport {
        let response = export_import::export_config(State(self.state.clone()))
            .await
            .unwrap()
            .into_response();
        let bytes = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
            .await
            .unwrap();
        let mut config: ConfigExport = serde_json::from_slice(&bytes).unwrap();
        config.users.clear();
        config
    }

    async fn selections(&self) -> usize {
        self.container
            .config_service
            .list_credential_selections()
            .await
            .unwrap()
            .len()
    }
}

fn fixed(id: &str) -> CredentialSelection {
    CredentialSelection::Fixed {
        credential_id: id.into(),
    }
}

fn streamer_owner(id: &str) -> CredentialOwner {
    CredentialOwner::Streamer {
        streamer_id: id.into(),
    }
}

#[tokio::test]
async fn selection_changes_through_the_api_reach_the_next_resolution() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let f = fixture().await;
        let a = f.profile("platform-huya", "huya=a").await;
        let b = f.profile("platform-huya", "huya=b").await;

        let (_, Json(created)) = streamers::create_streamer(
            State(streamers::StreamerRouteState::from_ref(&f.state)),
            Json(
                serde_json::from_value(json!({
                    "name": "Selected",
                    "url": "https://www.huya.com/1001",
                    "streamer_specific_config": {"credential_selection": {"mode": "fixed", "credential_id": a}, "record_danmu": true}
                }))
                .unwrap(),
            ),
        )
        .await
        .unwrap();
        let id = created.id.clone();
        assert_eq!(
            created.streamer_specific_config,
            Some(json!({"credential_selection": {"mode": "fixed", "credential_id": a}, "record_danmu": true}))
        );
        assert_eq!(f.resolved(&id).await, Some((streamer_owner(&id), fixed(&a))));

        // The row is identical after this write; only the selection changes.
        let response = f
            .update_streamer(&id, json!({"streamer_specific_config": {"credential_selection": {"mode": "fixed", "credential_id": b}, "record_danmu": true}}))
            .await;
        assert_eq!(response["streamer_specific_config"]["credential_selection"]["credential_id"], json!(b));
        assert_eq!(f.resolved(&id).await, Some((streamer_owner(&id), fixed(&b))));

        // A form without the selection keeps it; inherit removes it.
        f.update_streamer(&id, json!({"streamer_specific_config": {"record_danmu": false}}))
            .await;
        assert_eq!(f.resolved(&id).await, Some((streamer_owner(&id), fixed(&b))));
        let response = f
            .update_streamer(&id, json!({"streamer_specific_config": {"credential_selection": {"mode": "inherit"}, "record_danmu": false}}))
            .await;
        assert_eq!(response["streamer_specific_config"], json!({"record_danmu": false}));
        assert_eq!(f.resolved(&id).await, None);

        // A platform selection reaches its streamers, with pool defaults spelled out.
        let response = f
            .replace_platform("platform-huya", Some(json!({"mode": "pool", "credential_ids": [b, a]})))
            .await;
        let stored: Value =
            serde_json::from_str(response["credential_selection"].as_str().unwrap()).unwrap();
        assert_eq!(
            stored,
            json!({"mode": "pool", "credential_ids": [b, a], "strategy": "priority", "failover": true, "max_attempts": 3})
        );
        let platform = CredentialOwner::Platform {
            platform_id: "platform-huya".into(),
        };
        assert_eq!(f.resolved(&id).await.map(|(owner, _)| owner), Some(platform.clone()));
        // Saving the platform without the field keeps it.
        f.replace_platform("platform-huya", None).await;
        assert_eq!(f.resolved(&id).await.map(|(owner, _)| owner), Some(platform));

        // A template selection for the platform outranks the platform's.
        let (_, Json(template)) = templates::create_template(
            State(templates::TemplateRouteState::from_ref(&f.state)),
            Json(
                serde_json::from_value(json!({
                    "name": "Selecting",
                    "platform_overrides": {"huya": {"credential_selection": {"mode": "fixed", "credential_id": a}}}
                }))
                .unwrap(),
            ),
        )
        .await
        .unwrap();
        f.update_streamer(&id, json!({"template_id": template.id})).await;
        let template_owner = CredentialOwner::Template {
            template_id: template.id.clone(),
        };
        assert_eq!(f.resolved(&id).await, Some((template_owner.clone(), fixed(&a))));
        let response = f
            .update_template(&template.id, json!({"platform_overrides": {"huya": {"credential_selection": {"mode": "none"}}}}))
            .await;
        assert_eq!(response["platform_overrides"], json!({"huya": {"credential_selection": {"mode": "none"}}}));
        assert_eq!(f.resolved(&id).await, Some((template_owner, CredentialSelection::None)));

        // Moving to another platform drops the streamer's own selection, even
        // when the form repeats it.
        f.update_streamer(&id, json!({"streamer_specific_config": {"credential_selection": {"mode": "fixed", "credential_id": a}}}))
            .await;
        assert_eq!(f.resolved(&id).await, Some((streamer_owner(&id), fixed(&a))));
        let response = f
            .update_streamer(&id, json!({"url": "https://live.bilibili.com/42", "template_id": null, "streamer_specific_config": {"credential_selection": {"mode": "fixed", "credential_id": a}}}))
            .await;
        assert_eq!(response["platform_config_id"], "platform-bilibili");
        assert_eq!(response["streamer_specific_config"], json!({}));
        assert_eq!(f.resolved(&id).await, None);
    })
    .await
    .expect("selection changes must settle");
}

#[tokio::test]
async fn backups_carry_selections_through_replace_and_merge_imports() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let f = fixture().await;
        // Without accounts or selections the export keeps the older schema.
        assert_eq!(f.export().await.version, "0.1.8");

        let a = f.profile("platform-huya", "huya=a").await;
        let b = f.profile("platform-huya", "huya=b").await;
        f.replace_platform("platform-huya", Some(json!({"mode": "fixed", "credential_id": a})))
            .await;
        let (_, Json(template)) = templates::create_template(
            State(templates::TemplateRouteState::from_ref(&f.state)),
            Json(
                serde_json::from_value(json!({
                    "name": "Backed up",
                    "platform_overrides": {"huya": {"credential_selection": {"mode": "pool", "credential_ids": [b, a]}, "quality": 1}}
                }))
                .unwrap(),
            ),
        )
        .await
        .unwrap();
        let (_, Json(streamer)) = streamers::create_streamer(
            State(streamers::StreamerRouteState::from_ref(&f.state)),
            Json(
                serde_json::from_value(json!({
                    "name": "Backed up",
                    "url": "https://www.huya.com/1002",
                    "template_id": template.id,
                    "streamer_specific_config": {"credential_selection": {"mode": "none"}}
                }))
                .unwrap(),
            ),
        )
        .await
        .unwrap();

        let bundle = f.export().await;
        assert_eq!(bundle.version, "1.0.0");
        assert_eq!(bundle.credential_profiles.len(), 2);
        let huya = bundle
            .platforms
            .iter()
            .find(|platform| platform.platform_name == "huya")
            .unwrap();
        assert_eq!(huya.credential_selection, Some(fixed(&a)));
        assert_eq!(
            bundle.templates[0].platform_overrides,
            Some(json!({"huya": {"credential_selection": {"mode": "pool", "credential_ids": [b, a], "strategy": "priority", "failover": true, "max_attempts": 3}, "quality": 1}}))
        );
        assert_eq!(
            bundle.streamers[0].streamer_specific_config,
            Some(json!({"credential_selection": {"mode": "none"}}))
        );

        // Replace restores exactly what the bundle holds.
        let service = &f.state.configuration_import_service;
        f.update_streamer(&streamer.id, json!({"streamer_specific_config": {"credential_selection": {"mode": "inherit"}}}))
            .await;
        f.replace_platform("platform-huya", Some(json!({"mode": "inherit"})))
            .await;
        assert_eq!(f.selections().await, 1);
        service.import(bundle.clone(), ImportMode::Replace).await.unwrap();
        assert_eq!(f.selections().await, 3);
        let restored = f.export().await;
        assert_eq!(
            serde_json::to_value(&restored.platforms).unwrap(),
            serde_json::to_value(&bundle.platforms).unwrap()
        );
        assert_eq!(restored.templates[0].platform_overrides, bundle.templates[0].platform_overrides);
        assert_eq!(
            restored.streamers[0].streamer_specific_config,
            bundle.streamers[0].streamer_specific_config
        );
        let streamer_id = f
            .container
            .streamer_manager
            .get_streamer_by_url("https://www.huya.com/1002")
            .unwrap()
            .id;
        assert_eq!(
            f.resolved(&streamer_id).await,
            Some((streamer_owner(&streamer_id), CredentialSelection::None))
        );

        // Merge keeps a selection the bundle omits and applies the ones it carries.
        let mut merge = bundle.clone();
        for platform in &mut merge.platforms {
            platform.credential_selection = None;
        }
        merge.templates[0].platform_overrides = Some(json!({"huya": {"quality": 2}}));
        merge.streamers[0].streamer_specific_config =
            Some(json!({"credential_selection": {"mode": "fixed", "credential_id": b}}));
        service.import(merge, ImportMode::Merge).await.unwrap();
        let merged = f.export().await;
        assert_eq!(
            serde_json::to_value(&merged.platforms).unwrap(),
            serde_json::to_value(&bundle.platforms).unwrap()
        );
        assert_eq!(
            merged.templates[0].platform_overrides,
            Some(json!({"huya": {"credential_selection": {"mode": "pool", "credential_ids": [b, a], "strategy": "priority", "failover": true, "max_attempts": 3}, "quality": 2}}))
        );
        assert_eq!(
            f.resolved(&streamer_id).await,
            Some((streamer_owner(&streamer_id), fixed(&b)))
        );

        // A bundle written before selections existed replaces them all.
        let mut legacy = bundle;
        legacy.version = "0.1.8".into();
        legacy.credential_profiles.clear();
        for platform in &mut legacy.platforms {
            platform.credential_selection = None;
        }
        legacy.templates[0].platform_overrides = Some(json!({"huya": {"quality": 3}}));
        legacy.streamers[0].streamer_specific_config = None;
        service.import(legacy, ImportMode::Replace).await.unwrap();
        assert_eq!(f.selections().await, 0);
        let streamer_id = f
            .container
            .streamer_manager
            .get_streamer_by_url("https://www.huya.com/1002")
            .unwrap()
            .id;
        assert_eq!(f.resolved(&streamer_id).await, None);
        // Profiles are not part of an older bundle, so they stay.
        assert_eq!(f.export().await.credential_profiles.len(), 2);
    })
    .await
    .expect("backup imports must settle");
}
