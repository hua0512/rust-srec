use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;
use sqlx::SqlitePool;
use tokio::sync::{Notify, mpsc};

use super::*;
use crate::config::{ConfigService, ConfigUpdateEvent};
use crate::database::filter_store::{FilterSnapshotCache, FilterStore};
use crate::database::models::StreamerDbModel;
use crate::database::repositories::{
    FilterRepository, SqlxConfigRepository, SqlxFilterRepository, SqlxStreamerRepository,
    StreamerRepository,
};
use crate::domain::filter::Filter;

struct PausedFilterRepository {
    inner: Arc<SqlxFilterRepository>,
    paused_reads: usize,
    reads: AtomicUsize,
    updates: AtomicUsize,
    observed: mpsc::Sender<FilterDbModel>,
    resume: Notify,
}

impl PausedFilterRepository {
    fn new(
        inner: Arc<SqlxFilterRepository>,
        paused_reads: usize,
    ) -> (Arc<Self>, mpsc::Receiver<FilterDbModel>) {
        let (observed, receiver) = mpsc::channel(1);
        (
            Arc::new(Self {
                inner,
                paused_reads,
                reads: AtomicUsize::new(0),
                updates: AtomicUsize::new(0),
                observed,
                resume: Notify::new(),
            }),
            receiver,
        )
    }
}

#[async_trait]
impl FilterRepository for PausedFilterRepository {
    fn filter_snapshot_cache(&self) -> Option<FilterSnapshotCache> {
        self.inner.filter_snapshot_cache()
    }

    async fn get_filter(&self, id: &str) -> crate::Result<FilterDbModel> {
        let row = self.inner.get_filter(id).await?;
        if self.reads.fetch_add(1, Ordering::SeqCst) < self.paused_reads {
            self.observed.send(row.clone()).await.unwrap();
            self.resume.notified().await;
        }
        Ok(row)
    }

    async fn get_filters_for_streamer(&self, id: &str) -> crate::Result<Vec<FilterDbModel>> {
        self.inner.get_filters_for_streamer(id).await
    }

    async fn create_filter(&self, row: &FilterDbModel) -> crate::Result<()> {
        self.inner.create_filter(row).await
    }

    async fn update_filter(&self, row: &FilterDbModel) -> crate::Result<()> {
        self.inner.update_filter(row).await
    }

    async fn update_filter_if_current(
        &self,
        expected: &FilterDbModel,
        replacement: &FilterDbModel,
        on_commit: FilterCommitHook,
    ) -> crate::Result<bool> {
        self.updates.fetch_add(1, Ordering::SeqCst);
        self.inner
            .update_filter_if_current(expected, replacement, on_commit)
            .await
    }

    async fn delete_filter(&self, id: &str) -> crate::Result<()> {
        self.inner.delete_filter(id).await
    }

    async fn delete_filters_for_streamer(&self, id: &str) -> crate::Result<()> {
        self.inner.delete_filters_for_streamer(id).await
    }
}

async fn fixture() -> (FilterRouteState, SqlitePool, Arc<SqlxFilterRepository>) {
    let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    crate::database::run_migrations(&pool).await.unwrap();
    let streamers = Arc::new(SqlxStreamerRepository::new(pool.clone(), pool.clone()));
    let mut owner =
        StreamerDbModel::new("Timezone", "https://example.test/timezone", "platform-huya");
    owner.id = "timezone-owner".into();
    streamers.create_streamer(&owner).await.unwrap();
    let filters = Arc::new(SqlxFilterRepository::new(pool.clone(), pool.clone()));
    let state = FilterRouteState {
        filter_repository: filters.clone(),
        config_service: Arc::new(ConfigService::new(
            Arc::new(SqlxConfigRepository::new(pool.clone(), pool.clone())),
            streamers,
        )),
    };
    (state, pool, filters)
}

fn schedule() -> Value {
    json!({"days_of_week":["Monday"],"start_time":"09:00","end_time":"17:00","extension":{"preserve":true}})
}

async fn local_filter(repository: &SqlxFilterRepository) -> FilterDbModel {
    let mut config = schedule();
    config["timezone"] = json!("local");
    let original = FilterDbModel::new("timezone-owner", FilterType::TimeBased, config.to_string());
    repository.create_filter(&original).await.unwrap();
    original
}

fn assert_update_event(events: &mut tokio::sync::broadcast::Receiver<ConfigUpdateEvent>) {
    assert!(matches!(
        events.try_recv().unwrap(),
        ConfigUpdateEvent::StreamerFiltersUpdated { streamer_id } if streamer_id == "timezone-owner"
    ));
    assert!(matches!(
        events.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
}

#[tokio::test]
async fn editor_patch_retries_without_restoring_a_concurrently_changed_timezone() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let (state, _pool, repository) = fixture().await;
        let original = local_filter(&repository).await;
        // This store shares the repository cache without registering with the
        // config service, so its refresh proves committed repository invalidation.
        let store = FilterStore::new(repository.clone());
        let initial = store.get("timezone-owner").await.unwrap();
        let (paused, mut observed) = PausedFilterRepository::new(repository.clone(), 1);
        let mut editor_state = state.clone();
        editor_state.filter_repository = paused.clone();
        let mut events = state.config_service.subscribe();
        let mut editor_config = schedule();
        editor_config["end_time"] = json!("19:00");
        let update = tokio::spawn(update_filter(
            State(editor_state),
            Path(("timezone-owner".into(), original.id.clone())),
            Json(UpdateFilterRequest {
                filter_type: Some("TIME_BASED".into()),
                config: Some(editor_config),
            }),
        ));
        let stale = observed.recv().await.unwrap();
        assert_eq!(stale.config, original.config);

        let mut changed_zone = schedule();
        changed_zone["timezone"] = json!("Europe/Madrid");
        let Json(concurrent_response) = update_filter(
            State(state.clone()),
            Path(("timezone-owner".into(), original.id.clone())),
            Json(UpdateFilterRequest {
                filter_type: None,
                config: Some(changed_zone),
            }),
        )
        .await
        .unwrap();
        assert_eq!(concurrent_response.config["timezone"], "Europe/Madrid");
        assert_update_event(&mut events);
        let concurrent = store.get("timezone-owner").await.unwrap();
        assert!(!Arc::ptr_eq(&initial, &concurrent));

        paused.resume.notify_one();
        let Json(response) = update.await.unwrap().unwrap();
        assert_eq!(response.config["timezone"], "Europe/Madrid");
        assert_eq!(response.config["end_time"], "19:00:00");
        assert_eq!(paused.reads.load(Ordering::SeqCst), 2);
        assert_eq!(paused.updates.load(Ordering::SeqCst), 2);
        assert_update_event(&mut events);
        let stored = repository.get_filter(&original.id).await.unwrap();
        assert_eq!(serde_json::from_str::<Value>(&stored.config).unwrap(), response.config);
        let current = store.get("timezone-owner").await.unwrap();
        assert!(!Arc::ptr_eq(&concurrent, &current));
        assert!(matches!(
            &current[0],
            Filter::TimeBased(filter) if filter.timezone.as_deref() == Some("Europe/Madrid")
                && filter.end_time == "19:00:00"
        ));
        assert!(matches!(&initial[0], Filter::TimeBased(filter) if filter.timezone.as_deref() == Some("local")));
    })
    .await
    .expect("concurrent timezone PATCH must settle");
}

#[tokio::test]
async fn editor_patch_returns_conflict_after_three_changes_without_events_or_cache_eviction() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let (mut state, _pool, repository) = fixture().await;
        let original = local_filter(&repository).await;
        let store = FilterStore::new(repository.clone());
        let (paused, mut observed) = PausedFilterRepository::new(repository.clone(), 3);
        state.filter_repository = paused.clone();
        let mut events = state.config_service.subscribe();
        let mut editor_config = schedule();
        editor_config["end_time"] = json!("19:00");
        let update = tokio::spawn(update_filter(
            State(state.clone()),
            Path(("timezone-owner".into(), original.id.clone())),
            Json(UpdateFilterRequest {
                filter_type: None,
                config: Some(editor_config),
            }),
        ));
        let mut snapshots = Vec::new();
        for zone in ["Europe/Madrid", "America/New_York", "Asia/Tokyo"] {
            let mut current = observed.recv().await.unwrap();
            let mut config: Value = serde_json::from_str(&current.config).unwrap();
            config["timezone"] = json!(zone);
            current.config = config.to_string();
            repository.update_filter(&current).await.unwrap();
            snapshots.push(store.get("timezone-owner").await.unwrap());
            paused.resume.notify_one();
        }

        let error = update.await.unwrap().unwrap_err();
        assert_eq!(error.status, StatusCode::CONFLICT);
        assert_eq!(paused.reads.load(Ordering::SeqCst), 3);
        assert_eq!(paused.updates.load(Ordering::SeqCst), 3);
        assert!(matches!(
            events.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
        let current = store.get("timezone-owner").await.unwrap();
        assert!(Arc::ptr_eq(snapshots.last().unwrap(), &current));
        let stored = repository.get_filter(&original.id).await.unwrap();
        let config: Value = serde_json::from_str(&stored.config).unwrap();
        assert_eq!(config["timezone"], "Asia/Tokyo");
        assert_eq!(config["end_time"], "17:00");
    })
    .await
    .expect("contended timezone PATCH must stop retrying");
}

#[tokio::test]
async fn editor_patch_rechecks_ownership_after_a_concurrent_reassignment() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let (mut state, pool, repository) = fixture().await;
        let original = local_filter(&repository).await;
        let mut other = StreamerDbModel::new(
            "Other timezone owner",
            "https://example.test/other-timezone",
            "platform-huya",
        );
        other.id = "other-timezone-owner".into();
        SqlxStreamerRepository::new(pool.clone(), pool)
            .create_streamer(&other)
            .await
            .unwrap();
        let store = FilterStore::new(repository.clone());
        let (paused, mut observed) = PausedFilterRepository::new(repository.clone(), 1);
        state.filter_repository = paused.clone();
        let mut events = state.config_service.subscribe();
        let update = tokio::spawn(update_filter(
            State(state.clone()),
            Path(("timezone-owner".into(), original.id.clone())),
            Json(UpdateFilterRequest {
                filter_type: None,
                config: Some(schedule()),
            }),
        ));
        let mut current = observed.recv().await.unwrap();
        current.streamer_id = other.id.clone();
        repository.update_filter(&current).await.unwrap();
        let moved = store.get(&other.id).await.unwrap();
        let empty = store.get("timezone-owner").await.unwrap();
        paused.resume.notify_one();

        assert_eq!(
            update.await.unwrap().unwrap_err().status,
            StatusCode::NOT_FOUND
        );
        assert_eq!(paused.reads.load(Ordering::SeqCst), 2);
        assert_eq!(paused.updates.load(Ordering::SeqCst), 1);
        assert!(matches!(
            events.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
        assert!(Arc::ptr_eq(&moved, &store.get(&other.id).await.unwrap()));
        assert!(Arc::ptr_eq(
            &empty,
            &store.get("timezone-owner").await.unwrap()
        ));
        let stored = repository.get_filter(&original.id).await.unwrap();
        assert_eq!(stored.streamer_id, other.id);
        assert_eq!(stored.config, original.config);
    })
    .await
    .expect("reassigned filter PATCH must settle");
}

#[tokio::test]
async fn editor_patch_rechecks_type_before_preserving_timezone_on_retry() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let (mut state, _pool, repository) = fixture().await;
        let original = local_filter(&repository).await;
        let (paused, mut observed) = PausedFilterRepository::new(repository.clone(), 1);
        state.filter_repository = paused.clone();
        let mut events = state.config_service.subscribe();
        let update = tokio::spawn(update_filter(
            State(state.clone()),
            Path(("timezone-owner".into(), original.id.clone())),
            Json(UpdateFilterRequest {
                filter_type: Some("TIME_BASED".into()),
                config: Some(schedule()),
            }),
        ));
        let mut current = observed.recv().await.unwrap();
        current.filter_type = "CRON".into();
        current.config = json!({"expression":"0 * * * * *","timezone":"Europe/Madrid"}).to_string();
        repository.update_filter(&current).await.unwrap();
        paused.resume.notify_one();

        let Json(response) = update.await.unwrap().unwrap();
        assert_eq!(response.filter_type, "TIME_BASED");
        assert!(response.config.get("timezone").is_none());
        assert_eq!(paused.reads.load(Ordering::SeqCst), 2);
        assert_eq!(paused.updates.load(Ordering::SeqCst), 2);
        assert_update_event(&mut events);
        let stored = repository.get_filter(&original.id).await.unwrap();
        assert_eq!(stored.filter_type, "TIME_BASED");
        assert!(
            serde_json::from_str::<Value>(&stored.config)
                .unwrap()
                .get("timezone")
                .is_none()
        );
    })
    .await
    .expect("changed filter type PATCH must settle");
}

#[tokio::test]
async fn unchanged_editor_patch_keeps_stored_timezone_but_null_explicitly_selects_utc() {
    let (state, _pool, repository) = fixture().await;
    for zone in ["local", "Europe/Madrid", "UTC"] {
        let mut config = schedule();
        config["timezone"] = json!(zone);
        let original =
            FilterDbModel::new("timezone-owner", FilterType::TimeBased, config.to_string());
        repository.create_filter(&original).await.unwrap();
        let mut events = state.config_service.subscribe();
        let Json(response) = update_filter(
            State(state.clone()),
            Path(("timezone-owner".into(), original.id.clone())),
            Json(UpdateFilterRequest {
                filter_type: Some("TIME_BASED".into()),
                config: Some(schedule()),
            }),
        )
        .await
        .unwrap();
        assert_eq!(response.config["timezone"], zone);
        assert_eq!(response.config["start_time"], "09:00:00");
        assert_eq!(response.config["extension"], json!({"preserve":true}));
        assert!(
            matches!(events.try_recv().unwrap(),ConfigUpdateEvent::StreamerFiltersUpdated { streamer_id } if streamer_id=="timezone-owner")
        );
        let stored: Value =
            serde_json::from_str(&repository.get_filter(&original.id).await.unwrap().config)
                .unwrap();
        assert_eq!(stored["timezone"], zone);
        let mut utc = schedule();
        utc["timezone"] = Value::Null;
        let Json(response) = update_filter(
            State(state.clone()),
            Path(("timezone-owner".into(), original.id.clone())),
            Json(UpdateFilterRequest {
                filter_type: None,
                config: Some(utc),
            }),
        )
        .await
        .unwrap();
        assert!(response.config["timezone"].is_null());
    }
}

#[tokio::test]
async fn creates_and_type_changes_use_utc_omissions_and_reject_invalid_explicit_zones() {
    let (state, _pool, repository) = fixture().await;
    let (_, Json(created)) = create_filter(
        State(state.clone()),
        Path("timezone-owner".into()),
        Json(CreateFilterRequest {
            streamer_id: "timezone-owner".into(),
            filter_type: "TIME_BASED".into(),
            config: schedule(),
        }),
    )
    .await
    .unwrap();
    assert!(created.config.get("timezone").is_none());
    assert_eq!(created.config["extension"], json!({"preserve":true}));
    let cron = FilterDbModel::new(
        "timezone-owner",
        FilterType::Cron,
        json!({"expression":"0 * * * * *","timezone":"Europe/Madrid"}).to_string(),
    );
    repository.create_filter(&cron).await.unwrap();
    let Json(changed) = update_filter(
        State(state.clone()),
        Path(("timezone-owner".into(), cron.id.clone())),
        Json(UpdateFilterRequest {
            filter_type: Some("TIME_BASED".into()),
            config: Some(schedule()),
        }),
    )
    .await
    .unwrap();
    assert!(changed.config.get("timezone").is_none());
    for kind in ["TIME_BASED", "CRON"] {
        for zone in ["local", "invalid/zone", ""] {
            let mut config = if kind == "TIME_BASED" {
                schedule()
            } else {
                json!({"expression":"0 * * * * *","extension":{"preserve":true}})
            };
            config["timezone"] = json!(zone);
            let result = create_filter(
                State(state.clone()),
                Path("timezone-owner".into()),
                Json(CreateFilterRequest {
                    streamer_id: "timezone-owner".into(),
                    filter_type: kind.into(),
                    config,
                }),
            )
            .await;
            if zone == "local" {
                let (_, Json(response)) = result.unwrap();
                assert_eq!(response.config["timezone"], "local");
                assert_eq!(response.config["extension"], json!({"preserve":true}));
            } else {
                assert!(result.is_err());
            }
        }
    }
}
