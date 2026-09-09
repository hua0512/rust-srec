use super::*;
use crate::config::{ConfigService, ConfigUpdateEvent};
use crate::database::models::{FilterDbModel, FilterType, StreamerDbModel};
use crate::database::repositories::{
    SqlxConfigRepository, SqlxFilterRepository, SqlxStreamerRepository, StreamerRepository,
};
use crate::utils::task_supervisor::TaskSupervisor;

async fn fixture() -> (
    sqlx::SqlitePool,
    Arc<SqlxFilterRepository>,
    Arc<FilterStore>,
    Arc<TaskSupervisor>,
    Arc<crate::database::CommittedWriter>,
) {
    let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    crate::database::run_migrations(&pool).await.unwrap();
    let streamer_repo = SqlxStreamerRepository::new(pool.clone(), pool.clone());
    for id in ["one", "two"] {
        let mut row =
            StreamerDbModel::new(id, format!("https://example.test/{id}"), "platform-twitch");
        row.id = id.into();
        streamer_repo.create_streamer(&row).await.unwrap();
    }
    let tasks = Arc::new(TaskSupervisor::new());
    let writer =
        Arc::new(crate::database::CommittedWriter::new(pool.clone(), tasks.clone()).unwrap());
    let repository = Arc::new(
        SqlxFilterRepository::new(pool.clone(), pool.clone()).with_committed_writer(writer.clone()),
    );
    let store = Arc::new(FilterStore::new(repository.clone()));
    (pool, repository, store, tasks, writer)
}

fn filter(id: &str, text: &str) -> FilterDbModel {
    FilterDbModel::new(
        id,
        FilterType::Keyword,
        serde_json::json!({"include":[text], "exclude":[]}).to_string(),
    )
}

fn keyword(snapshot: &Snapshot) -> &str {
    let Filter::Keyword(filter) = &snapshot[0] else {
        panic!("expected keyword")
    };
    &filter.include[0]
}

#[tokio::test]
async fn repository_mutations_invalidate_every_affected_owner_only_after_success() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let (pool, repository, store, tasks, _writer) = fixture().await;
        let empty = store.get("one").await.unwrap();
        assert!(empty.is_empty());
        let mut row = filter("one", "created");
        repository.create_filter(&row).await.unwrap();
        let created = store.get("one").await.unwrap();
        assert_eq!(keyword(&created), "created");
        assert!(!Arc::ptr_eq(&empty, &created));
        assert!(repository.create_filter(&row).await.is_err());
        assert!(Arc::ptr_eq(&created, &store.get("one").await.unwrap()), "failed insertion must not invalidate");
        row.config = serde_json::json!({"include":["updated"],"exclude":[]}).to_string();
        repository.update_filter(&row).await.unwrap();
        let updated = store.get("one").await.unwrap();
        assert_eq!(keyword(&updated), "updated");
        sqlx::query("CREATE TRIGGER reject_filter BEFORE UPDATE ON filters BEGIN SELECT RAISE(ABORT, 'injected failure'); END")
            .execute(&pool).await.unwrap();
        row.config = serde_json::json!({"include":["rolled-back"],"exclude":[]}).to_string();
        assert!(repository.update_filter(&row).await.is_err());
        assert!(Arc::ptr_eq(&updated, &store.get("one").await.unwrap()));
        sqlx::query("DROP TRIGGER reject_filter").execute(&pool).await.unwrap();
        let other_empty = store.get("two").await.unwrap();
        row.streamer_id = "two".into();
        repository.update_filter(&row).await.unwrap();
        assert!(store.get("one").await.unwrap().is_empty());
        let moved = store.get("two").await.unwrap();
        assert_eq!(keyword(&moved), "rolled-back");
        assert!(!Arc::ptr_eq(&other_empty, &moved));
        repository.delete_filter(&row.id).await.unwrap();
        assert!(store.get("two").await.unwrap().is_empty());
        repository.create_filter(&filter("two", "one")).await.unwrap();
        repository.create_filter(&filter("two", "two")).await.unwrap();
        assert_eq!(store.get("two").await.unwrap().len(), 2);
        repository.delete_filters_for_streamer("two").await.unwrap();
        assert!(store.get("two").await.unwrap().is_empty());
        tasks.shutdown(Duration::from_secs(2)).await;
    }).await.expect("committed filter mutation contracts must settle");
}

#[tokio::test]
async fn configuration_notifications_reconcile_filter_snapshots_without_changing_old_readers() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let (pool, repository, store, tasks, _writer) = fixture().await;
        let service = ConfigService::new(
            Arc::new(SqlxConfigRepository::new(pool.clone(), pool.clone())),
            Arc::new(SqlxStreamerRepository::new(pool.clone(), pool.clone())),
        )
        .with_filter_store(store.clone());
        assert!(Arc::ptr_eq(
            &store,
            &service.filter_store_for(repository.clone())
        ));
        let row = filter("one", "initial");
        repository.create_filter(&row).await.unwrap();
        let old = store.get("one").await.unwrap();
        let mut events = service.subscribe();
        for (text, import) in [("changed", false), ("imported", true)] {
            sqlx::query("UPDATE filters SET config = ? WHERE id = ?")
                .bind(serde_json::json!({"include":[text], "exclude":[]}).to_string())
                .bind(&row.id)
                .execute(&pool)
                .await
                .unwrap();
            if import {
                service.notify_import_committed();
            } else {
                service.notify_streamer_filters_updated("one");
            }
            let event = events.recv().await.unwrap();
            if import {
                assert!(matches!(event, ConfigUpdateEvent::GlobalUpdated));
            } else {
                assert!(matches!(event, ConfigUpdateEvent::StreamerFiltersUpdated { streamer_id } if streamer_id == "one"));
            }
            assert_eq!(keyword(&store.get("one").await.unwrap()), text);
            assert_eq!(keyword(&old), "initial");
        }
        sqlx::query("DELETE FROM streamers WHERE id = 'one'")
            .execute(&pool)
            .await
            .unwrap();
        service.invalidate_filter_snapshots("one");
        assert!(store.get("one").await.unwrap().is_empty());
        // The same broad invalidation is used by GlobalUpdated and broadcast-lag
        // reconciliation even when rows were changed by an out-of-band writer.
        let row = filter("two", "before-global");
        repository.create_filter(&row).await.unwrap();
        store.get("two").await.unwrap();
        sqlx::query("DELETE FROM filters WHERE streamer_id = 'two'")
            .execute(&pool)
            .await
            .unwrap();
        let global = service.get_global_config().await.unwrap();
        service.update_global_config(&global).await.unwrap();
        assert!(matches!(
            events.recv().await.unwrap(),
            ConfigUpdateEvent::GlobalUpdated
        ));
        assert!(store.get("two").await.unwrap().is_empty());
        tasks.shutdown(Duration::from_secs(2)).await;
    })
    .await
    .expect("configuration invalidation must precede rechecks");
}

#[tokio::test]
async fn sqlite_order_and_invalid_filter_handling_match_the_monitor_contract() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let (_pool, repository, store, tasks, _writer) = fixture().await;
        repository
            .create_filter(&FilterDbModel::new(
                "one",
                FilterType::TimeBased,
                r#"{"days_of_week":[],"start_time":"00:00","end_time":"23:59","timezone":"UTC"}"#,
            ))
            .await
            .unwrap();
        repository
            .create_filter(&filter("one", "keyword"))
            .await
            .unwrap();
        repository
            .create_filter(&FilterDbModel::new("one", FilterType::Regex, "not-json"))
            .await
            .unwrap();
        repository
            .create_filter(&FilterDbModel::new(
                "one",
                FilterType::Category,
                r#"{"categories":["Games"]}"#,
            ))
            .await
            .unwrap();
        let snapshot = store.get("one").await.unwrap();
        assert_eq!(snapshot.len(), 3);
        assert!(matches!(&snapshot[0], Filter::Category(_)));
        assert!(matches!(&snapshot[1], Filter::Keyword(_)));
        assert!(matches!(&snapshot[2], Filter::TimeBased(_)));
        assert!(Arc::ptr_eq(&snapshot, &store.get("one").await.unwrap()));
        tasks.shutdown(Duration::from_secs(2)).await;
    })
    .await
    .expect("SQLite filter ordering must be preserved");
}

#[tokio::test]
async fn cancelled_mutation_and_dropped_repository_still_commit_and_invalidate() {
    use crate::database::committed_writer::{CommitPhase, CommitTestGate};
    tokio::time::timeout(Duration::from_secs(20), async {
        for phase in [CommitPhase::BeforeCommit, CommitPhase::AfterCommit] {
            let (pool, repository, store, tasks, writer) = fixture().await;
            let mut row = filter("one", "before");
            repository.create_filter(&row).await.unwrap();
            let old = store.get("one").await.unwrap();
            let cache = repository.filter_snapshot_cache().unwrap();
            let repository_lifetime = Arc::downgrade(&repository);
            row.config = serde_json::json!({"include":["committed"],"exclude":[]}).to_string();
            let gate = Arc::new(CommitTestGate::default());
            writer.set_commit_gate(phase, Some(gate.clone()));
            let caller_repo = repository.clone();
            let caller = tokio::spawn(async move { caller_repo.update_filter(&row).await });
            gate.started.notified().await;
            caller.abort();
            assert!(caller.await.unwrap_err().is_cancelled());
            drop(store);
            drop(repository);
            drop(writer);
            drop(tasks);
            assert!(
                repository_lifetime.upgrade().is_none(),
                "the admitted mutation must outlive its repository handle"
            );
            gate.release.notify_one();
            // The owned task retains the sole lease through its synchronous
            // invalidation, so acquiring it proves publication has finished.
            let mut connection = pool.acquire().await.unwrap();
            let committed: String =
                sqlx::query_scalar("SELECT config FROM filters WHERE streamer_id = 'one'")
                    .fetch_one(&mut *connection)
                    .await
                    .unwrap();
            assert!(committed.contains("committed"));
            assert!(matches!(cache.lookup("one"), Lookup::Missing));
            assert_eq!(keyword(&old), "before");
        }
    })
    .await
    .expect("cancelled callers cannot abandon commit-time invalidation");
}

#[tokio::test]
async fn standalone_filter_repository_supports_owned_invalidation_with_multiple_connections() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let directory = tempfile::tempdir().unwrap();
        let url = format!(
            "sqlite:{}?mode=rwc",
            directory.path().join("filters.sqlite").display()
        );
        let pool = crate::database::init_pool_with_size(&url, 2).await.unwrap();
        crate::database::run_migrations(&pool).await.unwrap();
        let mut streamer = StreamerDbModel::new(
            "Standalone",
            "https://example.test/standalone",
            "platform-twitch",
        );
        streamer.id = "standalone".into();
        SqlxStreamerRepository::new(pool.clone(), pool.clone())
            .create_streamer(&streamer)
            .await
            .unwrap();
        let repository = Arc::new(SqlxFilterRepository::new(pool.clone(), pool.clone()));
        let store = FilterStore::new(repository.clone());
        assert!(store.get("standalone").await.unwrap().is_empty());
        repository
            .create_filter(&filter("standalone", "multi-connection"))
            .await
            .unwrap();
        assert_eq!(
            keyword(&store.get("standalone").await.unwrap()),
            "multi-connection"
        );
        drop(store);
        drop(repository);
        pool.close().await;
    })
    .await
    .expect("commutative invalidation does not require single-writer row publication");
}
