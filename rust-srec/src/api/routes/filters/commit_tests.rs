use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Method, Request};
use serde_json::json;
use sqlx::SqlitePool;
use tokio::sync::broadcast;
use tower::ServiceExt;

use super::*;
use crate::config::{ConfigService, ConfigUpdateEvent};
use crate::database::CommittedWriter;
use crate::database::committed_writer::{CommitPhase, CommitTestGate};
use crate::database::filter_store::FilterStore;
use crate::database::models::StreamerDbModel;
use crate::database::repositories::{
    FilterRepository, SqlxConfigRepository, SqlxFilterRepository, SqlxStreamerRepository,
    StreamerRepository,
};
use crate::domain::filter::Filter;
use crate::utils::task_supervisor::TaskSupervisor;

#[derive(Clone, Copy, Debug)]
enum Mutation {
    Create,
    Update,
    Delete,
}

impl Mutation {
    const ALL: [Self; 3] = [Self::Create, Self::Update, Self::Delete];

    fn request(self, id: &str) -> Request<Body> {
        let (method, path, body) = match self {
            Self::Create => (
                Method::POST,
                "/streamers/filter-owner/filters".to_owned(),
                json!({"streamer_id":"filter-owner","filter_type":"KEYWORD","config":{"include":["after"],"exclude":[]}}),
            ),
            Self::Update => (
                Method::PATCH,
                format!("/streamers/filter-owner/filters/{id}"),
                json!({"config":{"include":["after"],"exclude":[]}}),
            ),
            Self::Delete => (
                Method::DELETE,
                format!("/streamers/filter-owner/filters/{id}"),
                Value::Null,
            ),
        };
        Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    fn sql_operation(self) -> &'static str {
        match self {
            Self::Create => "INSERT",
            Self::Update => "UPDATE",
            Self::Delete => "DELETE",
        }
    }
}

struct Fixture {
    state: FilterRouteState,
    pool: SqlitePool,
    repository: Arc<SqlxFilterRepository>,
    writer: Arc<CommittedWriter>,
    tasks: Arc<TaskSupervisor>,
    store: FilterStore,
    original: FilterDbModel,
}

async fn fixture(mutation: Mutation) -> Fixture {
    let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    crate::database::run_migrations(&pool).await.unwrap();
    let streamers = Arc::new(SqlxStreamerRepository::new(pool.clone(), pool.clone()));
    let mut owner = StreamerDbModel::new(
        "Filter commit owner",
        "https://example.test/filter-commit",
        "platform-huya",
    );
    owner.id = "filter-owner".into();
    streamers.create_streamer(&owner).await.unwrap();
    let tasks = Arc::new(TaskSupervisor::new());
    let writer = Arc::new(CommittedWriter::new(pool.clone(), tasks.clone()).unwrap());
    let repository = Arc::new(
        SqlxFilterRepository::new(pool.clone(), pool.clone()).with_committed_writer(writer.clone()),
    );
    let original = FilterDbModel::new(
        "filter-owner",
        FilterType::Keyword,
        json!({"include":["before"],"exclude":[]}).to_string(),
    );
    if !matches!(mutation, Mutation::Create) {
        repository.create_filter(&original).await.unwrap();
    }
    let state = FilterRouteState {
        filter_repository: repository.clone(),
        config_service: Arc::new(ConfigService::new(
            Arc::new(SqlxConfigRepository::new(pool.clone(), pool.clone())),
            streamers,
        )),
    };
    // Leave this store unregistered with ConfigService to distinguish repository
    // invalidation from the route hook's config-service invalidation fallback.
    let store = FilterStore::new(repository.clone());
    Fixture {
        state,
        pool,
        repository,
        writer,
        tasks,
        store,
        original,
    }
}

fn routes(state: FilterRouteState) -> Router {
    Router::new()
        .route("/streamers/{streamer_id}/filters", post(create_filter))
        .route(
            "/streamers/{streamer_id}/filters/{id}",
            patch(update_filter).delete(delete_filter),
        )
        .with_state(state)
}

fn assert_one_event(events: &mut broadcast::Receiver<ConfigUpdateEvent>) {
    assert!(matches!(
        events.try_recv().unwrap(),
        ConfigUpdateEvent::StreamerFiltersUpdated { streamer_id } if streamer_id == "filter-owner"
    ));
    assert!(
        events.try_recv().is_err(),
        "one committed route mutation publishes once"
    );
}

async fn assert_committed(fixture: &Fixture, mutation: Mutation) {
    let rows = fixture
        .repository
        .get_filters_for_streamer("filter-owner")
        .await
        .unwrap();
    let current = fixture.store.get("filter-owner").await.unwrap();
    if matches!(mutation, Mutation::Delete) {
        assert!(rows.is_empty());
        assert!(current.is_empty());
    } else {
        assert_eq!(rows.len(), 1);
        assert_eq!(
            serde_json::from_str::<Value>(&rows[0].config).unwrap()["include"],
            json!(["after"])
        );
        assert!(matches!(&current[0], Filter::Keyword(filter) if filter.include == ["after"]));
    }
}

#[tokio::test]
async fn cancelled_filter_routes_finish_commit_cache_invalidation_and_one_scheduler_event() {
    tokio::time::timeout(Duration::from_secs(30), async {
        for mutation in Mutation::ALL {
            for phase in [CommitPhase::BeforeCommit, CommitPhase::AfterCommit] {
                let fixture = fixture(mutation).await;
                let old = fixture.store.get("filter-owner").await.unwrap();
                let mut events = fixture.state.config_service.subscribe();
                let gate = Arc::new(CommitTestGate::default());
                fixture.writer.set_commit_gate(phase, Some(gate.clone()));
                let caller = tokio::spawn(
                    routes(fixture.state.clone()).oneshot(mutation.request(&fixture.original.id)),
                );
                gate.started.notified().await;
                caller.abort();
                assert!(caller.await.unwrap_err().is_cancelled());
                assert!(events.try_recv().is_err());
                assert!(Arc::ptr_eq(
                    &old,
                    &fixture.store.get("filter-owner").await.unwrap()
                ));
                gate.release.notify_one();

                // The writer retains its sole lease until synchronous publication
                // finishes. Acquiring it is a deterministic completion barrier.
                drop(fixture.pool.acquire().await.unwrap());
                assert_one_event(&mut events);
                assert_committed(&fixture, mutation).await;
                assert!(!Arc::ptr_eq(
                    &old,
                    &fixture.store.get("filter-owner").await.unwrap()
                ));
                if !matches!(mutation, Mutation::Create) {
                    assert!(
                        matches!(&old[0], Filter::Keyword(filter) if filter.include == ["before"])
                    );
                }
                assert!(fixture.tasks.shutdown(Duration::from_secs(2)).await);
            }
        }
    })
    .await
    .expect("cancelled filter route publication must settle");
}

#[tokio::test]
async fn successful_filter_routes_publish_once_and_failed_writes_leave_snapshots_unchanged() {
    tokio::time::timeout(Duration::from_secs(30), async {
        for mutation in Mutation::ALL {
            let fixture = fixture(mutation).await;
            let old = fixture.store.get("filter-owner").await.unwrap();
            let mut events = fixture.state.config_service.subscribe();
            let reject = format!(
                "CREATE TRIGGER reject_filter BEFORE {} ON filters BEGIN SELECT RAISE(ABORT, 'injected failure'); END",
                mutation.sql_operation(),
            );
            // The interpolated operation is one of three fixed fixture keywords.
            sqlx::query(sqlx::AssertSqlSafe(reject)).execute(&fixture.pool).await.unwrap();
            let failed = routes(fixture.state.clone()).oneshot(mutation.request(&fixture.original.id)).await.unwrap();
            assert_eq!(failed.status(), StatusCode::INTERNAL_SERVER_ERROR);
            assert!(events.try_recv().is_err());
            assert!(Arc::ptr_eq(&old, &fixture.store.get("filter-owner").await.unwrap()));
            sqlx::query("DROP TRIGGER reject_filter").execute(&fixture.pool).await.unwrap();

            let success = routes(fixture.state.clone()).oneshot(mutation.request(&fixture.original.id)).await.unwrap();
            assert!(success.status().is_success());
            assert_one_event(&mut events);
            assert_committed(&fixture, mutation).await;
            assert!(fixture.tasks.shutdown(Duration::from_secs(2)).await);
        }
    }).await.expect("filter route success and failure publication must settle");
}
