//! Real SQLite integration tests for the `monitor_event_outbox` `ON DELETE SET NULL`
//! migration, `NotificationOutboxDrainer` two-level decoding, shutdown replay, and
//! dead-letter settlement.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use jiff::Timestamp;
use rust_srec::database::models::{
    ChannelType, NotificationChannelDbModel, StreamerDbModel, WebhookChannelSettings,
};
use rust_srec::database::repositories::{
    MonitorOutboxTxOps, NotificationRepository, SqlxNotificationRepository, SqlxStreamerRepository,
    StreamerRepository as _,
};
use rust_srec::domain::StreamerState;
use rust_srec::domain::streamer::FatalErrorType;
use rust_srec::notification::{
    NotificationEvent, NotificationOutboxDrainer, NotificationService, NotificationServiceConfig,
    OutboxDrainReport,
};
use rust_srec::recorder::{OutboxEvent, StreamOnlineKind};
use rust_srec::services::maintenance::{MaintenanceConfig, MaintenanceScheduler};
use serde::Serialize;
use serde_json::Value;
use sqlx::SqlitePool;
use tokio::net::TcpListener;
use tokio::sync::{Mutex, Notify, oneshot};
use tokio_util::sync::CancellationToken;

/// Wire-format fixture matching pre-v2 `monitor_event_outbox` rows.
#[derive(Serialize)]
enum LegacyMonitorEvent {
    StreamerLive {
        streamer_id: String,
        session_id: String,
        streamer_name: String,
        streamer_url: String,
        title: String,
        category: Option<String>,
        streams: Vec<Value>,
        media_headers: Option<Value>,
        media_extras: Option<Value>,
        timestamp: Timestamp,
    },
    StreamerOffline {
        streamer_id: String,
        streamer_name: String,
        session_id: Option<String>,
        timestamp: Timestamp,
    },
    FatalError {
        streamer_id: String,
        streamer_name: String,
        error_type: FatalErrorType,
        message: String,
        new_state: StreamerState,
        timestamp: Timestamp,
    },
    TransientError {
        streamer_id: String,
        streamer_name: String,
        error_message: String,
        consecutive_errors: i32,
        timestamp: Timestamp,
    },
    ChecksFailing {
        streamer_id: String,
        streamer_name: String,
        platform: String,
        last_error: String,
        consecutive_errors: i32,
        timestamp: Timestamp,
    },
    ChecksRestored {
        streamer_id: String,
        streamer_name: String,
        timestamp: Timestamp,
    },
    StateChanged {
        streamer_id: String,
        streamer_name: String,
        old_state: StreamerState,
        new_state: StreamerState,
        reason: Option<String>,
        timestamp: Timestamp,
    },
}

const OUTBOX_SET_NULL_MIGRATION_SQL: &str =
    include_str!("../../migrations/20261124120000_monitor_event_outbox_set_null.sql");

/// Create an in-memory SQLite DB migrated through `last` (inclusive).
async fn database_through(last: &str) -> SqlitePool {
    let directory = tempfile::tempdir().unwrap();
    let legacy_migrations = directory.path().join("legacy-migrations");
    std::fs::create_dir(&legacy_migrations).unwrap();
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".sql")
            && name
                .split('_')
                .next()
                .is_some_and(|version| version <= last)
        {
            std::fs::copy(entry.path(), legacy_migrations.join(name)).unwrap();
        }
    }
    let pool = rust_srec::database::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    sqlx::migrate::Migrator::new(legacy_migrations.as_path())
        .await
        .unwrap()
        .run(&pool)
        .await
        .unwrap();
    pool
}

async fn insert_streamer(pool: &SqlitePool, id: &str, name: &str) {
    let mut streamer = StreamerDbModel::new(
        name.to_owned(),
        format!("https://example.test/{id}"),
        "platform-twitch",
    );
    streamer.id = id.to_owned();
    SqlxStreamerRepository::new(pool.clone(), pool.clone())
        .create_streamer(&streamer)
        .await
        .unwrap();
}

async fn enqueue_legacy_event(
    tx: &mut sqlx::SqliteConnection,
    streamer_id: &str,
    event: &LegacyMonitorEvent,
) {
    let event_type = match event {
        LegacyMonitorEvent::StreamerLive { .. } => "StreamerLive",
        LegacyMonitorEvent::StreamerOffline { .. } => "StreamerOffline",
        LegacyMonitorEvent::FatalError { .. } => "FatalError",
        LegacyMonitorEvent::TransientError { .. } => "TransientError",
        LegacyMonitorEvent::ChecksFailing { .. } => "ChecksFailing",
        LegacyMonitorEvent::ChecksRestored { .. } => "ChecksRestored",
        LegacyMonitorEvent::StateChanged { .. } => "StateChanged",
    };
    let payload = serde_json::to_string(event).unwrap();
    MonitorOutboxTxOps::enqueue_raw(
        tx,
        Some(streamer_id),
        event_type,
        &payload,
        rust_srec::database::time::now_ms(),
    )
    .await
    .unwrap();
}

async fn enqueue_outbox_event(
    tx: &mut sqlx::SqliteConnection,
    streamer_id: Option<&str>,
    event: &OutboxEvent,
    created_at_ms: i64,
) {
    let payload = serde_json::to_string(event).unwrap();
    MonitorOutboxTxOps::enqueue_raw(tx, streamer_id, event.event_type(), &payload, created_at_ms)
        .await
        .unwrap();
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HookMode {
    Ok,
    Fail,
    /// Accept requests until `hold_on_attempt` (1-indexed), hold that request open
    /// until `release_held` fires, then respond 503.
    HoldAt {
        hold_on_attempt: usize,
    },
}

struct HookServerState {
    mode: Mutex<HookMode>,
    received: Mutex<Vec<Value>>,
    attempts: AtomicUsize,
    held_arrived: Notify,
    release_held: Mutex<Option<oneshot::Receiver<()>>>,
}

async fn webhook_handler(
    State(state): State<Arc<HookServerState>>,
    Json(body): Json<Value>,
) -> StatusCode {
    let attempt = state.attempts.fetch_add(1, Ordering::SeqCst) + 1;
    let mode = *state.mode.lock().await;
    match mode {
        HookMode::Ok => {
            state.received.lock().await.push(body);
            StatusCode::OK
        }
        HookMode::Fail => StatusCode::INTERNAL_SERVER_ERROR,
        HookMode::HoldAt { hold_on_attempt } if attempt == hold_on_attempt => {
            let rx = state.release_held.lock().await.take();
            state.held_arrived.notify_one();
            if let Some(rx) = rx {
                let _ = rx.await;
            }
            StatusCode::SERVICE_UNAVAILABLE
        }
        HookMode::HoldAt { .. } => {
            state.received.lock().await.push(body);
            StatusCode::OK
        }
    }
}

async fn spawn_webhook_server(
    initial_mode: HookMode,
) -> (String, Arc<HookServerState>, tokio::task::JoinHandle<()>) {
    let state = Arc::new(HookServerState {
        mode: Mutex::new(initial_mode),
        received: Mutex::new(Vec::new()),
        attempts: AtomicUsize::new(0),
        held_arrived: Notify::new(),
        release_held: Mutex::new(None),
    });
    let app = Router::new()
        .route("/hook", post(webhook_handler))
        .with_state(state.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}/hook"), state, handle)
}

async fn register_webhook_channel(
    repo: &SqlxNotificationRepository,
    url: &str,
    events: &[&str],
) -> String {
    let settings = WebhookChannelSettings {
        url: url.to_owned(),
        headers: None,
        method: "POST".to_owned(),
        enabled: Some(true),
        timeout_secs: Some(5),
        auth: None,
    };
    let mut settings_json = serde_json::to_value(&settings).unwrap();
    settings_json["min_priority"] = Value::String("low".to_owned());
    let channel = NotificationChannelDbModel::new(
        "integration-webhook",
        ChannelType::Webhook,
        serde_json::to_string(&settings_json).unwrap(),
    );
    let channel_id = channel.id.clone();
    repo.create_channel(&channel).await.unwrap();
    for event_name in events {
        repo.subscribe(&channel_id, event_name).await.unwrap();
    }
    channel_id
}

#[tokio::test]
async fn migration_rebuilds_outbox_with_set_null_and_preserves_rows_and_retention() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let pool = database_through("20261122120000").await;
        insert_streamer(&pool, "s-keep", "Kept Streamer").await;
        insert_streamer(&pool, "s-reap", "Reaped Streamer").await;

        let now_ms = rust_srec::database::time::now_ms();
        let old_delivered_ms = now_ms - 48 * 60 * 60 * 1000;
        let recent_delivered_ms = now_ms - 60 * 60 * 1000;

        let reap_fatal_payload = serde_json::to_string(&LegacyMonitorEvent::FatalError {
            streamer_id: "s-reap".into(),
            streamer_name: "Reaped Streamer".into(),
            error_type: FatalErrorType::NotFound,
            message: "channel gone".into(),
            new_state: StreamerState::NotFound,
            timestamp: Timestamp::from_millisecond(now_ms).unwrap(),
        })
        .unwrap();
        let keep_failing_payload = serde_json::to_string(&LegacyMonitorEvent::ChecksFailing {
            streamer_id: "s-keep".into(),
            streamer_name: "Kept Streamer".into(),
            platform: "twitch".into(),
            last_error: "503".into(),
            consecutive_errors: 3,
            timestamp: Timestamp::from_millisecond(now_ms).unwrap(),
        })
        .unwrap();

        let seeded = [
            (
                10i64,
                "s-reap",
                "StateChanged",
                "{}",
                old_delivered_ms - 1000,
                Some(old_delivered_ms),
                1i64,
                None::<&str>,
            ),
            (
                20i64,
                "s-reap",
                "StateChanged",
                "{}",
                recent_delivered_ms - 1000,
                Some(recent_delivered_ms),
                1i64,
                None::<&str>,
            ),
            (
                30i64,
                "s-reap",
                "FatalError",
                reap_fatal_payload.as_str(),
                now_ms - 5000,
                None,
                2i64,
                Some("transient error"),
            ),
            (
                40i64,
                "s-keep",
                "ChecksFailing",
                keep_failing_payload.as_str(),
                now_ms - 1000,
                None,
                0i64,
                None::<&str>,
            ),
        ];

        for (id, sid, etype, payload, created, delivered, attempts, last_err) in seeded {
            sqlx::query(
                "INSERT INTO monitor_event_outbox \
                 (id, streamer_id, event_type, payload, created_at, delivered_at, attempts, last_error) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(id)
            .bind(sid)
            .bind(etype)
            .bind(payload)
            .bind(created)
            .bind(delivered)
            .bind(attempts)
            .bind(last_err)
            .execute(&pool)
            .await
            .unwrap();
        }

        // Run migration through latest, then replay the raw `-- no-transaction` SQL
        // script itself to verify crash-recovery replay safety per CONTRIBUTING.md.
        rust_srec::database::run_migrations(&pool).await.unwrap();
        sqlx::raw_sql(OUTBOX_SET_NULL_MIGRATION_SQL)
            .execute(&pool)
            .await
            .unwrap();
        let fk_enabled: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(fk_enabled, 1, "migration must leave PRAGMA foreign_keys=ON");

        // All rows, IDs, payloads, timestamps, and metadata are preserved verbatim.
        #[derive(sqlx::FromRow)]
        struct OutboxRow {
            id: i64,
            streamer_id: Option<String>,
            event_type: String,
            payload: String,
            created_at: i64,
            delivered_at: Option<i64>,
            attempts: i64,
            last_error: Option<String>,
        }
        let rows: Vec<OutboxRow> = sqlx::query_as(
            "SELECT id, streamer_id, event_type, payload, created_at, delivered_at, attempts, last_error \
             FROM monitor_event_outbox ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0].id, 10);
        assert_eq!(rows[0].delivered_at, Some(old_delivered_ms));
        assert_eq!(rows[1].id, 20);
        assert_eq!(rows[1].delivered_at, Some(recent_delivered_ms));
        assert_eq!(rows[2].id, 30);
        assert_eq!(rows[2].streamer_id.as_deref(), Some("s-reap"));
        assert_eq!(rows[2].event_type, "FatalError");
        assert_eq!(rows[2].payload, reap_fatal_payload);
        assert_eq!(rows[2].created_at, now_ms - 5000);
        assert_eq!(rows[2].delivered_at, None);
        assert_eq!(rows[2].attempts, 2);
        assert_eq!(rows[2].last_error.as_deref(), Some("transient error"));
        assert_eq!(rows[3].id, 40);
        assert_eq!(rows[3].streamer_id.as_deref(), Some("s-keep"));

        // Undelivered scan index is intact.
        let idx_sql: Option<String> = sqlx::query_scalar(
            "SELECT sql FROM sqlite_schema \
             WHERE type = 'index' AND name = 'monitor_event_outbox_undelivered'",
        )
        .fetch_optional(&pool)
        .await
        .unwrap();
        assert!(
            idx_sql
                .as_deref()
                .is_some_and(|sql| sql.contains("delivered_at") && sql.contains("id")),
            "expected monitor_event_outbox_undelivered index, got {idx_sql:?}"
        );

        assert!(
            sqlx::query("PRAGMA foreign_key_check")
                .fetch_all(&pool)
                .await
                .unwrap()
                .is_empty()
        );
        let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(integrity, "ok");

        // Reaping a deleted streamer clears FK to NULL across all its rows without
        // deleting them or losing JSON payload fields, and SQL replay still preserves NULL rows.
        sqlx::query("DELETE FROM streamers WHERE id = 's-reap'")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::raw_sql(OUTBOX_SET_NULL_MIGRATION_SQL)
            .execute(&pool)
            .await
            .unwrap();

        let post_reap_fks: Vec<(i64, Option<String>)> =
            sqlx::query_as("SELECT id, streamer_id FROM monitor_event_outbox ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(
            post_reap_fks,
            vec![
                (10, None),
                (20, None),
                (30, None),
                (40, Some("s-keep".to_owned())),
            ]
        );
        let reaped_payload: String =
            sqlx::query_scalar("SELECT payload FROM monitor_event_outbox WHERE id = 30")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(reaped_payload.contains("s-reap"));
        assert!(reaped_payload.contains("Reaped Streamer"));

        // 24h delivered_at cleanup prunes row 10 (NULL streamer_id, >24h delivered)
        // while retaining row 20 (<24h delivered), row 30 (undelivered, NULL FK), and row 40.
        let cancel = CancellationToken::new();
        let scheduler = Arc::new(MaintenanceScheduler::new(
            pool.clone(),
            pool.clone(),
            MaintenanceConfig::default(),
        ));
        let handle = scheduler.start(cancel.clone());
        let mut last_remaining = Vec::new();
        let pruned = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let remaining: Vec<i64> =
                    sqlx::query_scalar("SELECT id FROM monitor_event_outbox ORDER BY id")
                        .fetch_all(&pool)
                        .await
                        .unwrap();
                if remaining == vec![20, 30, 40] {
                    return;
                }
                last_remaining = remaining;
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        cancel.cancel();
        let _ = handle.await;
        assert!(
            pruned.is_ok(),
            "expected maintenance to leave rows [20, 30, 40], last observed: {last_remaining:?}"
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
    .expect("outbox migration integration test must finish");
}

#[tokio::test]
async fn drainer_delivers_mixed_legacy_and_v2_in_id_order_and_handles_deleted_streamers() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let pool = rust_srec::database::init_pool_with_size("sqlite::memory:", 1)
            .await
            .unwrap();
        rust_srec::database::run_migrations(&pool).await.unwrap();
        insert_streamer(&pool, "s-active", "Active Streamer").await;
        insert_streamer(&pool, "s-deleted", "Deleted Streamer").await;

        let (url, server_state, server_handle) = spawn_webhook_server(HookMode::Ok).await;
        let repo = Arc::new(SqlxNotificationRepository::new(pool.clone(), pool.clone()));
        register_webhook_channel(
            &repo,
            &url,
            &[
                "stream_online",
                "stream_offline",
                "fatal_error",
                "streamer_checks_failing",
                "streamer_checks_restored",
            ],
        )
        .await;

        let service = Arc::new(NotificationService::with_repository(
            NotificationServiceConfig {
                max_retries: 2,
                initial_retry_delay_ms: 5,
                max_retry_delay_ms: 10,
                ..Default::default()
            },
            repo.clone(),
        ));
        service.reload_from_db().await.unwrap();

        let base_ms = 1_735_100_000_000i64;
        let mut tx = rust_srec::database::begin_immediate(&pool).await.unwrap();

        // 1. Legacy non-notifying StreamerLive -> no-op
        enqueue_legacy_event(
            &mut tx,
            "s-active",
            &LegacyMonitorEvent::StreamerLive {
                streamer_id: "s-active".into(),
                session_id: "legacy-sess".into(),
                streamer_name: "Active Streamer".into(),
                streamer_url: "https://example.test/s-active".into(),
                title: "Legacy".into(),
                category: None,
                streams: Vec::new(),
                media_headers: None,
                media_extras: None,
                timestamp: Timestamp::from_millisecond(base_ms + 1).unwrap(),
            },
        )
        .await;

        // 2. Legacy alertable FatalError -> delivered (#1)
        enqueue_legacy_event(
            &mut tx,
            "s-active",
            &LegacyMonitorEvent::FatalError {
                streamer_id: "s-active".into(),
                streamer_name: "Active Streamer".into(),
                error_type: FatalErrorType::Banned,
                message: "legacy ban".into(),
                new_state: StreamerState::FatalError,
                timestamp: Timestamp::from_millisecond(base_ms + 2).unwrap(),
            },
        )
        .await;

        // 3. V2 StreamOnline -> delivered (#2)
        enqueue_outbox_event(
            &mut tx,
            Some("s-active"),
            &OutboxEvent::StreamOnline {
                streamer_id: "s-active".into(),
                streamer_name: "Active Streamer".into(),
                session_id: "v2-sess".into(),
                title: "V2 Live".into(),
                category: Some("Music".into()),
                started_at_ms: base_ms + 3,
                kind: StreamOnlineKind::Created,
            },
            base_ms + 3,
        )
        .await;

        // 4-6. Legacy non-notifying StateChanged / StreamerOffline / TransientError -> no-op
        enqueue_legacy_event(
            &mut tx,
            "s-active",
            &LegacyMonitorEvent::StateChanged {
                streamer_id: "s-active".into(),
                streamer_name: "Active Streamer".into(),
                old_state: StreamerState::NotLive,
                new_state: StreamerState::OutOfSchedule,
                reason: Some("filter".into()),
                timestamp: Timestamp::from_millisecond(base_ms + 4).unwrap(),
            },
        )
        .await;
        enqueue_legacy_event(
            &mut tx,
            "s-active",
            &LegacyMonitorEvent::StreamerOffline {
                streamer_id: "s-active".into(),
                streamer_name: "Active Streamer".into(),
                session_id: Some("legacy-sess".into()),
                timestamp: Timestamp::from_millisecond(base_ms + 5).unwrap(),
            },
        )
        .await;
        enqueue_legacy_event(
            &mut tx,
            "s-active",
            &LegacyMonitorEvent::TransientError {
                streamer_id: "s-active".into(),
                streamer_name: "Active Streamer".into(),
                error_message: "blip".into(),
                consecutive_errors: 1,
                timestamp: Timestamp::from_millisecond(base_ms + 6).unwrap(),
            },
        )
        .await;

        // 7. Malformed poison JSON row -> skipped so head-of-line progression continues
        sqlx::query(
            "INSERT INTO monitor_event_outbox (streamer_id, event_type, payload, created_at) \
             VALUES ('s-active', 'Corrupt', '{not-json', ?)",
        )
        .bind(base_ms + 7)
        .execute(&mut *tx)
        .await
        .unwrap();

        // 8. Legacy ChecksFailing for soon-deleted streamer -> delivered (#3)
        enqueue_legacy_event(
            &mut tx,
            "s-deleted",
            &LegacyMonitorEvent::ChecksFailing {
                streamer_id: "s-deleted".into(),
                streamer_name: "Deleted Streamer".into(),
                platform: "twitch".into(),
                last_error: "timeout".into(),
                consecutive_errors: 3,
                timestamp: Timestamp::from_millisecond(base_ms + 8).unwrap(),
            },
        )
        .await;

        // 9. V2 ChecksFailing -> delivered (#4)
        enqueue_outbox_event(
            &mut tx,
            Some("s-active"),
            &OutboxEvent::ChecksFailing {
                streamer_id: "s-active".into(),
                streamer_name: "Active Streamer".into(),
                platform: "twitch".into(),
                last_error: "v2 timeout".into(),
                consecutive_errors: 3,
                occurred_at_ms: base_ms + 9,
            },
            base_ms + 9,
        )
        .await;

        // 10. V2 FatalError -> delivered (#5)
        enqueue_outbox_event(
            &mut tx,
            Some("s-active"),
            &OutboxEvent::FatalError {
                streamer_id: "s-active".into(),
                streamer_name: "Active Streamer".into(),
                error_type: FatalErrorType::NotFound,
                message: "v2 fatal reason".into(),
                new_state: StreamerState::NotFound,
                occurred_at_ms: base_ms + 10,
            },
            base_ms + 10,
        )
        .await;

        // 11. V2 StreamOffline for soon-deleted streamer -> delivered (#6)
        enqueue_outbox_event(
            &mut tx,
            Some("s-deleted"),
            &OutboxEvent::StreamOffline {
                streamer_id: "s-deleted".into(),
                streamer_name: "Deleted Streamer".into(),
                session_id: "del-sess".into(),
                duration_secs: Some(123.5),
                ended_at_ms: base_ms + 11,
            },
            base_ms + 11,
        )
        .await;

        // 12. V2 ChecksRestored -> delivered (#7)
        enqueue_outbox_event(
            &mut tx,
            Some("s-active"),
            &OutboxEvent::ChecksRestored {
                streamer_id: "s-active".into(),
                streamer_name: "Active Streamer".into(),
                occurred_at_ms: base_ms + 12,
            },
            base_ms + 12,
        )
        .await;

        // 13. Legacy ChecksRestored -> delivered (#8)
        enqueue_legacy_event(
            &mut tx,
            "s-active",
            &LegacyMonitorEvent::ChecksRestored {
                streamer_id: "s-active".into(),
                streamer_name: "Active Streamer".into(),
                timestamp: Timestamp::from_millisecond(base_ms + 13).unwrap(),
            },
        )
        .await;

        tx.commit().await.unwrap();

        // Physically reap `s-deleted` before draining: FK becomes NULL while rows survive.
        sqlx::query("DELETE FROM streamers WHERE id = 's-deleted'")
            .execute(&pool)
            .await
            .unwrap();

        let drainer =
            NotificationOutboxDrainer::new(pool.clone(), service.clone(), CancellationToken::new());
        let report = drainer.drain_until_wait().await.unwrap();
        assert_eq!(
            report,
            OutboxDrainReport {
                fetched: 13,
                delivered: 8,
                skipped: 5,
                failed: 0,
                needs_wait: false,
            }
        );

        // Webhook observed ONLY the 8 alertable notifications in strict `id` ascending order
        // with exact millisecond timestamps and StreamOffline duration_secs preserved.
        let received = server_state.received.lock().await;
        let summary: Vec<(&str, &str, i64)> = received
            .iter()
            .map(|body| {
                let ts_ms = body["timestamp"]
                    .as_str()
                    .unwrap()
                    .parse::<Timestamp>()
                    .unwrap()
                    .as_millisecond();
                (
                    body["event_type"].as_str().unwrap(),
                    body["streamer_id"].as_str().unwrap(),
                    ts_ms,
                )
            })
            .collect();
        assert_eq!(
            summary,
            vec![
                ("fatal_error", "s-active", base_ms + 2),
                ("stream_online", "s-active", base_ms + 3),
                ("streamer_checks_failing", "s-deleted", base_ms + 8),
                ("streamer_checks_failing", "s-active", base_ms + 9),
                ("fatal_error", "s-active", base_ms + 10),
                ("stream_offline", "s-deleted", base_ms + 11),
                ("streamer_checks_restored", "s-active", base_ms + 12),
                ("streamer_checks_restored", "s-active", base_ms + 13),
            ]
        );
        assert_eq!(
            received[5]["data"]["StreamOffline"]["duration_secs"],
            Value::from(123.5)
        );
        drop(received);

        // Also exercise the race where a streamer is deleted AFTER fetch (so the caller
        // passes a non-NULL `streamer_id`): `SqlxNotificationRepository::add_event_log`
        // must atomically resolve the FK column to NULL without an FK violation while
        // preserving original streamer_id and streamer_name in JSON payload.
        let raced_ts = base_ms + 14;
        service
            .deliver_durable_for_outbox(
                NotificationEvent::FatalError {
                    streamer_id: "s-deleted".into(),
                    streamer_name: "Deleted Streamer".into(),
                    error_type: "NotFound".into(),
                    message: "raced delete".into(),
                    timestamp: Timestamp::from_millisecond(raced_ts).unwrap(),
                },
                Some("s-deleted"),
            )
            .await
            .unwrap();

        let log_rows: Vec<(String, Option<String>, i64, String)> = sqlx::query_as(
            "SELECT event_type, streamer_id, created_at, payload \
             FROM notification_event_log ORDER BY rowid",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(log_rows.len(), 9);
        assert_eq!(log_rows[0].1.as_deref(), Some("s-active"));
        assert_eq!(log_rows[0].2, base_ms + 2);
        assert_eq!(log_rows[2].1, None);
        assert_eq!(log_rows[2].2, base_ms + 8);
        assert!(log_rows[2].3.contains("s-deleted"));
        assert!(log_rows[2].3.contains("Deleted Streamer"));
        assert_eq!(log_rows[5].1, None);
        assert_eq!(log_rows[5].2, base_ms + 11);
        assert!(log_rows[5].3.contains("s-deleted"));
        assert!(log_rows[5].3.contains("Deleted Streamer"));
        assert_eq!(log_rows[8].0, "fatal_error");
        assert_eq!(log_rows[8].1, None);
        assert_eq!(log_rows[8].2, raced_ts);
        assert!(log_rows[8].3.contains("s-deleted"));
        assert!(log_rows[8].3.contains("Deleted Streamer"));

        let undelivered: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM monitor_event_outbox WHERE delivered_at IS NULL",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(undelivered, 0);
        assert!(
            sqlx::query("PRAGMA foreign_key_check")
                .fetch_all(&pool)
                .await
                .unwrap()
                .is_empty()
        );

        service.stop().await;
        server_handle.abort();
    })
    .await
    .expect("mixed drainer integration test must finish");
}

#[tokio::test]
async fn drainer_preserves_head_of_line_on_shutdown_cancellation_and_dead_letter() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let pool = rust_srec::database::init_pool_with_size("sqlite::memory:", 1)
            .await
            .unwrap();
        rust_srec::database::run_migrations(&pool).await.unwrap();
        insert_streamer(&pool, "s1", "Replay Streamer").await;

        let (url, server_state, server_handle) =
            spawn_webhook_server(HookMode::HoldAt { hold_on_attempt: 2 }).await;
        let repo = Arc::new(SqlxNotificationRepository::new(pool.clone(), pool.clone()));
        let channel_id =
            register_webhook_channel(&repo, &url, &["stream_online", "fatal_error"]).await;

        let make_service = || {
            let repo = repo.clone();
            async move {
                let svc = Arc::new(NotificationService::with_repository(
                    NotificationServiceConfig {
                        max_retries: 2,
                        initial_retry_delay_ms: 5,
                        max_retry_delay_ms: 10,
                        ..Default::default()
                    },
                    repo,
                ));
                svc.reload_from_db().await.unwrap();
                svc
            }
        };

        // Part A: Partial-batch flush + head-of-line preservation on service shutdown.
        // Enqueue 3 rows: row 1 settles 200 OK, row 2 is held when stop() fires, row 3 must not run.
        let t1 = 1_735_200_000_000i64;
        let mut tx = rust_srec::database::begin_immediate(&pool).await.unwrap();
        for (offset, title) in [(0, "First"), (1000, "Second"), (2000, "Third")] {
            enqueue_outbox_event(
                &mut tx,
                Some("s1"),
                &OutboxEvent::StreamOnline {
                    streamer_id: "s1".into(),
                    streamer_name: "Replay Streamer".into(),
                    session_id: format!("sess-{offset}"),
                    title: title.into(),
                    category: None,
                    started_at_ms: t1 + offset,
                    kind: StreamOnlineKind::Created,
                },
                t1 + offset,
            )
            .await;
        }
        tx.commit().await.unwrap();

        let (release_tx, release_rx) = oneshot::channel();
        *server_state.release_held.lock().await = Some(release_rx);
        let service1 = make_service().await;
        let drainer1 = NotificationOutboxDrainer::new(
            pool.clone(),
            service1.clone(),
            CancellationToken::new(),
        );
        let arrived = server_state.held_arrived.notified();
        let drain_task = tokio::spawn(async move { drainer1.drain_once().await.unwrap() });

        arrived.await;
        service1.stop().await;
        let _ = release_tx.send(());

        let report1 = drain_task.await.unwrap();
        assert_eq!(
            report1,
            OutboxDrainReport {
                fetched: 3,
                delivered: 1,
                skipped: 0,
                failed: 1,
                needs_wait: true,
            }
        );

        let states_after_stop: Vec<(Option<i64>, i64, Option<String>)> = sqlx::query_as(
            "SELECT delivered_at, attempts, last_error FROM monitor_event_outbox ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert!(
            states_after_stop[0].0.is_some(),
            "row 1 must stay delivered"
        );
        assert_eq!(states_after_stop[1], (None, 0, None));
        assert_eq!(states_after_stop[2], (None, 0, None));

        // Also test the drainer's own CancellationToken cancelling mid-delivery on row 2
        // before NotificationService::stop(), leaving rows 2 & 3 untouched.
        let (release_tx2, release_rx2) = oneshot::channel();
        *server_state.release_held.lock().await = Some(release_rx2);
        server_state.attempts.store(0, Ordering::SeqCst);
        *server_state.mode.lock().await = HookMode::HoldAt { hold_on_attempt: 1 };
        let service2 = make_service().await;
        let drainer_cancel = CancellationToken::new();
        let drainer2 =
            NotificationOutboxDrainer::new(pool.clone(), service2.clone(), drainer_cancel.clone());
        let arrived2 = server_state.held_arrived.notified();
        let drain_task2 = tokio::spawn(async move { drainer2.drain_once().await.unwrap() });
        arrived2.await;
        drainer_cancel.cancel();
        let _ = release_tx2.send(());
        let report2 = drain_task2.await.unwrap();
        assert_eq!(report2.delivered, 0);
        assert_eq!(report2.failed, 1);
        assert!(report2.needs_wait);

        // Fresh drainer replays ONLY rows 2 and 3 in order (row 1 is never duplicated).
        *server_state.mode.lock().await = HookMode::Ok;
        let drainer3 = NotificationOutboxDrainer::new(
            pool.clone(),
            service2.clone(),
            CancellationToken::new(),
        );
        let report3 = drainer3.drain_until_wait().await.unwrap();
        assert_eq!(report3.delivered, 2);
        assert_eq!(report3.failed, 0);
        let titles: Vec<String> = server_state
            .received
            .lock()
            .await
            .iter()
            .map(|body| {
                body["data"]["StreamOnline"]["title"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        assert_eq!(titles, vec!["First", "Second", "Third"]);

        // Part B: Transient dead-letter insert failure records attempts & last_error and
        // halts head-of-line progression; once DB recovers, dead-letter persistence succeeds
        // and settles both rows in order.
        *server_state.mode.lock().await = HookMode::Fail;
        server_state.attempts.store(0, Ordering::SeqCst);
        sqlx::query(
            "CREATE TRIGGER fail_dead_letter_insert BEFORE INSERT ON notification_dead_letter \
             BEGIN SELECT RAISE(ABORT, 'simulated dead-letter write failure'); END",
        )
        .execute(&pool)
        .await
        .unwrap();

        let t2 = 1_735_200_060_000i64;
        let mut tx = rust_srec::database::begin_immediate(&pool).await.unwrap();
        for (offset, msg) in [(0, "dead-letter-1"), (1000, "dead-letter-2")] {
            enqueue_outbox_event(
                &mut tx,
                Some("s1"),
                &OutboxEvent::FatalError {
                    streamer_id: "s1".into(),
                    streamer_name: "Replay Streamer".into(),
                    error_type: FatalErrorType::NotFound,
                    message: msg.into(),
                    new_state: StreamerState::NotFound,
                    occurred_at_ms: t2 + offset,
                },
                t2 + offset,
            )
            .await;
        }
        tx.commit().await.unwrap();

        let report_transient = drainer3.drain_once().await.unwrap();
        assert_eq!(report_transient.delivered, 0);
        assert_eq!(report_transient.failed, 1);
        assert!(report_transient.needs_wait);
        assert_eq!(server_state.attempts.load(Ordering::SeqCst), 2);

        let failing_rows: Vec<(Option<i64>, i64, Option<String>)> = sqlx::query_as(
            "SELECT delivered_at, attempts, last_error FROM monitor_event_outbox \
             WHERE delivered_at IS NULL ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(failing_rows.len(), 2);
        assert_eq!(failing_rows[0].0, None);
        assert_eq!(failing_rows[0].1, 1);
        assert!(
            failing_rows[0]
                .2
                .as_deref()
                .is_some_and(|e| e.contains("dead letter"))
        );
        assert_eq!(
            failing_rows[1],
            (None, 0, None),
            "row behind transient failure must not be attempted out of order"
        );

        // Drop fault trigger: retrying now persists dead letters and marks both rows delivered.
        sqlx::query("DROP TRIGGER fail_dead_letter_insert")
            .execute(&pool)
            .await
            .unwrap();
        server_state.attempts.store(0, Ordering::SeqCst);

        let report_dl = drainer3.drain_until_wait().await.unwrap();
        assert_eq!(report_dl.delivered, 2);
        assert_eq!(report_dl.failed, 0);
        assert_eq!(server_state.attempts.load(Ordering::SeqCst), 4);

        let dl_rows = repo.list_dead_letters(Some(&channel_id), 10).await.unwrap();
        assert_eq!(dl_rows.len(), 2);
        assert!(
            dl_rows
                .iter()
                .all(|r| r.event_name == "fatal_error" && r.retry_count == 2)
        );

        let remaining_undelivered: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM monitor_event_outbox WHERE delivered_at IS NULL",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(remaining_undelivered, 0);

        service2.stop().await;
        server_handle.abort();
    })
    .await
    .expect("head-of-line shutdown and dead-letter integration test must finish");
}
