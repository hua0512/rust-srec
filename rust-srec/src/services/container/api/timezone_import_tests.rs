use std::sync::Arc;
use std::time::Duration;

use axum::{extract::State, response::IntoResponse};
use serde_json::json;

use super::ServiceContainer;
use crate::config::backup::{ConfigExport, ImportMode, JobPresetExport};
use crate::database::models::{FilterDbModel, FilterType, StreamerDbModel};
use crate::domain::filter::Filter;

async fn export(state: crate::api::server::AppState) -> ConfigExport {
    let response = crate::api::routes::export_import::export_config(State(state))
        .await
        .unwrap()
        .into_response();
    let body = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&body).unwrap()
}

#[tokio::test]
async fn public_timezone_import_rolls_back_or_invalidates_filter_snapshots_after_commit() {
    tokio::time::timeout(Duration::from_secs(30),async {
        let directory=tempfile::tempdir().unwrap();
        let pool=crate::database::init_pool_with_size("sqlite::memory:",1).await.unwrap();
        crate::database::run_migrations(&pool).await.unwrap();
        let container=ServiceContainer::new(pool.clone(),pool.clone()).await.unwrap();
        let (logging,_layer)=crate::logging::LoggingConfig::for_route_tests(directory.path().to_owned());
        assert!(container.logging_config.set(Arc::new(logging)).is_ok());
        let state=container.build_api_state(None).unwrap();
        let mut owner=StreamerDbModel::new("Timezone import","https://example.test/timezone-import","platform-huya");
        owner.state="DISABLED".into();
        state.streamer_repository.create_streamer(&owner).await.unwrap();
        let body=json!({"days_of_week":["Monday"],"start_time":"09:00","end_time":"17:00","timezone":"UTC","extension":{"kept":true}});
        state.filter_repository.create_filter(&FilterDbModel::new(&owner.id,FilterType::TimeBased,body.to_string())).await.unwrap();
        // W24's shared store observes the same repository and commit invalidation
        // boundary as the production monitor. A failed import must not evict it.
        let filters=state.config_service.filter_store_for(state.filter_repository.clone());
        let cached=filters.get(&owner.id).await.unwrap();
        let mut bundle=export(state.clone()).await;
        bundle.version="0.1.7".into();
        bundle.users.clear();
        bundle.streamers[0].filters[0].config.as_object_mut().unwrap().remove("timezone");
        let mut events=container.event_broadcaster.subscribe();
        let mut failing=bundle.clone();
        failing.job_presets.push(JobPresetExport {name:"timezone-late-failure".into(),description:None,category:None,processor:"remux".into(),config:json!({})});
        sqlx::query("CREATE TRIGGER reject_timezone_import BEFORE INSERT ON job_presets WHEN NEW.name='timezone-late-failure' BEGIN SELECT RAISE(ABORT,'timezone import failure'); END").execute(&pool).await.unwrap();
        assert!(state.configuration_import_service.import(failing,ImportMode::Merge).await.is_err());
        assert!(events.try_recv().is_err());
        assert!(Arc::ptr_eq(&cached,&filters.get(&owner.id).await.unwrap()));
        let stored=state.filter_repository.get_filters_for_streamer(&owner.id).await.unwrap();
        assert_eq!(serde_json::from_str::<serde_json::Value>(&stored[0].config).unwrap(),body);

        state.configuration_import_service.import(bundle,ImportMode::Merge).await.unwrap();
        let local=filters.get(&owner.id).await.unwrap();
        assert!(!Arc::ptr_eq(&cached,&local));
        assert!(matches!(&local[0],Filter::TimeBased(filter) if filter.timezone.as_deref()==Some("local")));
        let mut current=export(state.clone()).await;
        assert_eq!(current.version,"0.1.8");
        assert_eq!(current.streamers[0].filters[0].config["timezone"],"local");
        assert_eq!(current.streamers[0].filters[0].config["extension"],json!({"kept":true}));
        current.streamers[0].filters[0].config.as_object_mut().unwrap().remove("timezone");
        current.users.clear();
        state.configuration_import_service.import(current,ImportMode::Merge).await.unwrap();
        let utc=filters.get(&owner.id).await.unwrap();
        assert!(!Arc::ptr_eq(&local,&utc));
        assert!(matches!(&utc[0],Filter::TimeBased(filter) if filter.timezone.is_none()));
        assert_eq!(export(state.clone()).await.streamers[0].filters[0].config["timezone"],"UTC");
        container.stream_monitor.stop();
        container.notification_service.stop().await;
        container.pipeline_manager.stop().await;
        container.cancellation_token.cancel();
        assert!(container.task_supervisor.shutdown(Duration::from_secs(1)).await);
    }).await.expect("timezone import/cache contract must settle");
}
