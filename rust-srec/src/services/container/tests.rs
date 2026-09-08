use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use tokio::io::AsyncWriteExt;
use tokio::sync::{Notify, mpsc};

use super::{
    RECOVERY_PROGRESS_MIN_BYTES, broadcast_error_is_recoverable,
    should_end_stream_on_danmu_stream_closed, should_record_recovery_from_progress,
};
use crate::danmu::test_support::FakeProvider;
use crate::danmu::{CollectionSpec, DanmuService, ProviderRegistry};
use crate::database::models::{LiveSessionDbModel, StreamerDbModel};
use crate::database::repositories::{
    SessionRepository, SqlxSessionRepository, SqlxStreamerRepository, StreamerRepository,
};
use crate::domain::StreamerState;
use crate::downloader::engine::{DownloadProgress, EngineStartError, EngineType};
use crate::downloader::{
    DownloadConfig, DownloadEngine, DownloadFailureKind, DownloadHandle, DownloadManagerConfig,
    DownloadManagerEvent, DownloadProgressEvent, DownloadProtocol, DownloadStopCause,
    DownloadTerminalEvent, EngineEndSignal, SegmentEvent, SegmentInfo,
};

const SHUTDOWN_STREAMER_ID: &str = "shutdown-tracer-streamer";
const SHUTDOWN_SESSION_ID: &str = "shutdown-tracer-session";
const OPEN_SEGMENT_BYTES: &[u8] = b"recording";
const FINAL_SEGMENT_BYTES: &[u8] = b"-finalized";
const EXPECTED_SEGMENT_BYTES: &[u8] = b"recording-finalized";

#[derive(Clone)]
struct ShutdownFlushEngine {
    segment_path: PathBuf,
    started: Arc<Notify>,
}

#[async_trait]
impl DownloadEngine for ShutdownFlushEngine {
    fn engine_type(&self) -> EngineType {
        EngineType::Ffmpeg
    }

    async fn run(&self, handle: Arc<DownloadHandle>) -> std::result::Result<(), EngineStartError> {
        let started_at = Utc::now();
        let mut file = tokio::fs::File::create(&self.segment_path)
            .await
            .map_err(|error| {
                EngineStartError::new(
                    DownloadFailureKind::Io,
                    format!("failed to create shutdown tracer segment: {error}"),
                )
            })?;
        file.write_all(OPEN_SEGMENT_BYTES).await.map_err(|error| {
            EngineStartError::new(
                DownloadFailureKind::Io,
                format!("failed to write shutdown tracer segment: {error}"),
            )
        })?;
        file.flush().await.map_err(|error| {
            EngineStartError::new(
                DownloadFailureKind::Io,
                format!("failed to flush shutdown tracer segment: {error}"),
            )
        })?;
        handle
            .event_tx
            .send(SegmentEvent::SegmentStarted {
                path: self.segment_path.clone(),
                sequence: 0,
                started_at,
            })
            .await
            .map_err(|error| {
                EngineStartError::new(
                    DownloadFailureKind::Other,
                    format!("failed to emit shutdown tracer segment start: {error}"),
                )
            })?;
        self.started.notify_one();

        handle.cancellation_token.cancelled().await;

        file.write_all(FINAL_SEGMENT_BYTES).await.map_err(|error| {
            EngineStartError::new(
                DownloadFailureKind::Io,
                format!("failed to finalize shutdown tracer segment: {error}"),
            )
        })?;
        file.flush().await.map_err(|error| {
            EngineStartError::new(
                DownloadFailureKind::Io,
                format!("failed to flush finalized shutdown tracer segment: {error}"),
            )
        })?;
        file.sync_all().await.map_err(|error| {
            EngineStartError::new(
                DownloadFailureKind::Io,
                format!("failed to sync finalized shutdown tracer segment: {error}"),
            )
        })?;
        drop(file);

        let size_bytes = tokio::fs::metadata(&self.segment_path)
            .await
            .map_err(|error| {
                EngineStartError::new(
                    DownloadFailureKind::Io,
                    format!("failed to stat finalized shutdown tracer segment: {error}"),
                )
            })?
            .len();
        handle
            .event_tx
            .send(SegmentEvent::SegmentCompleted(SegmentInfo {
                path: self.segment_path.clone(),
                duration_secs: 1.0,
                size_bytes,
                index: 0,
                started_at: Some(started_at),
                completed_at: Utc::now(),
                split_reason_code: None,
                split_reason_details_json: None,
            }))
            .await
            .map_err(|error| {
                EngineStartError::new(
                    DownloadFailureKind::Other,
                    format!("failed to emit shutdown tracer segment completion: {error}"),
                )
            })?;
        handle
            .event_tx
            .send(SegmentEvent::DownloadCompleted {
                total_bytes: size_bytes,
                total_duration_secs: 1.0,
                total_segments: 1,
                engine_signal: EngineEndSignal::CleanDisconnect,
            })
            .await
            .map_err(|error| {
                EngineStartError::new(
                    DownloadFailureKind::Other,
                    format!("failed to emit shutdown tracer terminal event: {error}"),
                )
            })?;
        Ok(())
    }

    fn is_available(&self) -> bool {
        true
    }

    fn version(&self) -> Option<String> {
        Some("shutdown-tracer".to_string())
    }
}

async fn migrated_test_pool() -> sqlx::SqlitePool {
    let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
        .await
        .expect("test database should initialize");
    crate::database::run_migrations(&pool)
        .await
        .expect("test migrations should succeed");
    pool
}

#[tokio::test]
async fn graceful_shutdown_flushes_and_persists_the_final_segment_before_pool_close() {
    let temp_dir = tempfile::tempdir().expect("test directory should initialize");
    let database_path = temp_dir.path().join("shutdown-tracer.sqlite");
    let database_url = format!("sqlite:{}?mode=rwc", database_path.to_string_lossy());
    let output_dir = temp_dir.path().join("recordings");
    let segment_path = output_dir.join("segment-0.flv");
    let segment_path_string = segment_path.to_string_lossy().into_owned();
    let expected_size = 19;
    let danmu_path = segment_path.with_extension("xml");

    let (pool, write_pool) = crate::database::init_database_pools(&database_url)
        .await
        .expect("test database pools should initialize");
    crate::database::run_migrations(&pool)
        .await
        .expect("test migrations should succeed");
    let paired_segment_pipeline = crate::database::models::DagPipelineDefinition::new(
        "Shutdown Paired Segment Pipeline",
        vec![crate::database::models::DagStep::new(
            "record-final-segment",
            crate::database::models::PipelineStep::Inline {
                processor: "execute".to_string(),
                config: serde_json::json!({
                    "command": if cfg!(windows) { "exit /B 0" } else { "true" }
                }),
            },
        )],
    );
    let paired_segment_pipeline_json =
        serde_json::to_string(&paired_segment_pipeline).expect("test pipeline should serialize");
    let updated = sqlx::query(
        "UPDATE global_config \
         SET min_segment_size_bytes = 0, auto_thumbnail = FALSE, record_danmu = TRUE, paired_segment_pipeline = ? \
         WHERE id = 'global-configuration'",
    )
    .bind(paired_segment_pipeline_json)
    .execute(&write_pool)
    .await
    .expect("shutdown tracer config should update");
    assert_eq!(updated.rows_affected(), 1);

    let mut streamer = StreamerDbModel::new(
        "Shutdown tracer",
        "https://example.com/shutdown-tracer",
        "platform-twitch",
    );
    streamer.id = SHUTDOWN_STREAMER_ID.to_string();
    streamer.state = StreamerState::Live.as_str().to_string();
    SqlxStreamerRepository::new(pool.clone(), write_pool.clone())
        .create_streamer(&streamer)
        .await
        .expect("shutdown tracer streamer should persist");

    let mut session = LiveSessionDbModel::new(SHUTDOWN_STREAMER_ID);
    session.id = SHUTDOWN_SESSION_ID.to_string();
    SqlxSessionRepository::new(pool.clone(), write_pool.clone())
        .create_session(&session)
        .await
        .expect("shutdown tracer session should persist");

    let mut container = super::ServiceContainer::with_full_config(
        pool,
        write_pool,
        super::ServiceContainerConfig {
            cache_ttl: Duration::from_secs(60),
            event_capacity: 8,
            download_config: DownloadManagerConfig::default(),
            pipeline_config: crate::pipeline::PipelineManagerConfig::default(),
            api_config: crate::api::server::ApiServerConfig::default(),
        },
    )
    .await
    .expect("service container should initialize");

    let (_danmu_items_tx, danmu_items_rx) = mpsc::channel(8);
    let mut providers = ProviderRegistry::new();
    providers.register(Arc::new(FakeProvider::new(vec![danmu_items_rx])));
    let (danmu_coordination_sender, danmu_coordination_receiver) =
        crate::danmu::events::danmu_coordination_channel();
    let danmu_service = Arc::new(
        DanmuService::with_providers(providers)
            .with_session_repository(container.session_repository.clone())
            .with_coordination_sender(danmu_coordination_sender.clone()),
    );
    container.danmu_service = danmu_service.clone();
    container.danmu_coordination_sender = danmu_coordination_sender;
    *container.danmu_coordination_receiver.lock() = Some(danmu_coordination_receiver);

    container.pipeline_manager.clone().start();
    container.setup_download_event_subscriptions();
    container.setup_session_lifecycle_subscriptions();
    container.setup_danmu_event_subscriptions();
    danmu_service
        .start_collection(CollectionSpec {
            session_id: SHUTDOWN_SESSION_ID.to_string(),
            streamer_id: SHUTDOWN_STREAMER_ID.to_string(),
            streamer_url: FakeProvider::URL.to_string(),
            cookies: None,
            extras: None,
            statistics: crate::domain::DanmuStatisticsConfig::default(),
        })
        .await
        .expect("shutdown tracer danmu collection should start");

    let started = Arc::new(Notify::new());
    container
        .download_manager
        .register_engine(Arc::new(ShutdownFlushEngine {
            segment_path: segment_path.clone(),
            started: started.clone(),
        }));
    let mut events = container.download_manager.subscribe();
    let download_id = container
        .download_manager
        .start_download(
            DownloadConfig::new(
                "https://example.invalid/live.flv",
                output_dir.clone(),
                SHUTDOWN_STREAMER_ID,
                "Shutdown tracer",
                SHUTDOWN_SESSION_ID,
            )
            .with_protocol(DownloadProtocol::Flv),
            Some("ffmpeg".to_string()),
            false,
        )
        .await
        .expect("shutdown tracer download should start");
    tokio::time::timeout(Duration::from_secs(5), started.notified())
        .await
        .expect("shutdown tracer engine should open its segment");
    assert_eq!(container.download_manager.active_count(), 1);

    // A collection that ended earlier in the run reaches `shutdown_until`
    // as a `runtime_failures` entry. Only `shutdown_failures` may make
    // `shutdown_with_grace_period` return `Err`.
    danmu_service.seed_runtime_failure_for_test(
        "danmu collection earlier-session failed: websocket connect timed out".to_string(),
    );

    tokio::time::timeout(
        Duration::from_secs(10),
        container.shutdown_with_grace_period(Duration::from_secs(10)),
    )
    .await
    .expect("service container shutdown should complete within its deadline")
    .expect("service container should shut down gracefully");

    assert_eq!(container.download_manager.active_count(), 0);
    assert!(container.pool.is_closed());
    assert!(container.write_pool.is_closed());
    assert_eq!(
        tokio::fs::read(&segment_path)
            .await
            .expect("finalized segment should exist"),
        EXPECTED_SEGMENT_BYTES
    );
    let xml = tokio::fs::read_to_string(&danmu_path)
        .await
        .expect("finalized danmu segment should exist");
    assert!(xml.trim_end().ends_with("</i>"));
    let renamed_segment_path = output_dir.join("segment-0-renamed.flv");
    tokio::fs::rename(&segment_path, &renamed_segment_path)
        .await
        .expect("finalized segment should be renamable after shutdown");
    tokio::fs::rename(&renamed_segment_path, &segment_path)
        .await
        .expect("renamed segment should move back to its persisted path");

    let mut segment_completed_positions = Vec::new();
    let mut terminal_positions = Vec::new();
    let mut event_position = 0;
    loop {
        match events.try_recv() {
            Ok(DownloadManagerEvent::Progress(DownloadProgressEvent::SegmentCompleted {
                download_id: event_download_id,
                session_id,
                segment_path: event_segment_path,
                segment_index,
                ..
            })) => {
                assert_eq!(event_download_id, download_id);
                assert_eq!(session_id, SHUTDOWN_SESSION_ID);
                assert_eq!(event_segment_path, segment_path_string);
                assert_eq!(segment_index, 0);
                segment_completed_positions.push(event_position);
            }
            Ok(DownloadManagerEvent::Terminal(DownloadTerminalEvent::Cancelled {
                download_id: event_download_id,
                session_id,
                cause,
                ..
            })) => {
                assert_eq!(event_download_id, download_id);
                assert_eq!(session_id, SHUTDOWN_SESSION_ID);
                assert_eq!(cause, DownloadStopCause::Shutdown);
                terminal_positions.push(event_position);
            }
            Ok(DownloadManagerEvent::Terminal(terminal)) => {
                panic!("unexpected shutdown tracer terminal event: {terminal:?}");
            }
            Ok(DownloadManagerEvent::Progress(_)) => {}
            Err(
                tokio::sync::broadcast::error::TryRecvError::Empty
                | tokio::sync::broadcast::error::TryRecvError::Closed,
            ) => break,
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(skipped)) => {
                panic!("shutdown tracer event receiver lagged by {skipped}");
            }
        }
        event_position += 1;
    }
    assert_eq!(segment_completed_positions.len(), 1);
    assert_eq!(terminal_positions.len(), 1);
    assert!(segment_completed_positions[0] < terminal_positions[0]);

    let (reopened_pool, reopened_write_pool) = crate::database::init_database_pools(&database_url)
        .await
        .expect("closed shutdown tracer database should reopen");
    let session_repo =
        SqlxSessionRepository::new(reopened_pool.clone(), reopened_write_pool.clone());

    let outputs = session_repo
        .get_media_outputs_for_session(SHUTDOWN_SESSION_ID)
        .await
        .expect("shutdown tracer outputs should load");
    assert_eq!(outputs.len(), 2);
    let video_output = outputs
        .iter()
        .find(|output| output.file_type == "VIDEO")
        .expect("video output should persist");
    assert_eq!(video_output.file_path, segment_path_string);
    assert_eq!(video_output.size_bytes, expected_size);
    let danmu_output = outputs
        .iter()
        .find(|output| output.file_type == "DANMU_XML")
        .expect("danmu XML output should persist");
    assert_eq!(
        danmu_output.file_path,
        danmu_path.to_string_lossy().into_owned()
    );
    assert!(danmu_output.size_bytes > 0);

    let segments = session_repo
        .list_session_segments_for_session(SHUTDOWN_SESSION_ID, 10)
        .await
        .expect("shutdown tracer segments should load");
    assert_eq!(segments.len(), 1);
    assert_eq!(segments[0].segment_index, 0);
    assert_eq!(segments[0].file_path, segment_path_string);
    assert_eq!(segments[0].size_bytes, expected_size);
    let next_segment_index = session_repo
        .next_session_segment_index(SHUTDOWN_SESSION_ID)
        .await
        .expect("next segment index should load");
    assert_eq!(next_segment_index, 1);
    let dag_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM dag_execution \
         WHERE session_id = ? AND dag_definition LIKE '%Shutdown Paired Segment Pipeline%'",
    )
    .bind(SHUTDOWN_SESSION_ID)
    .fetch_one(&reopened_pool)
    .await
    .expect("shutdown tracer DAG count should load");
    assert_eq!(
        dag_count, 1,
        "the final paired-segment DAG must be created once"
    );
    let all_dag_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM dag_execution WHERE session_id = ?")
            .bind(SHUTDOWN_SESSION_ID)
            .fetch_one(&reopened_pool)
            .await
            .expect("shutdown tracer total DAG count should load");
    assert_eq!(
        all_dag_count, 1,
        "shutdown must not duplicate the paired-segment DAG"
    );
    let dag_statuses: Vec<String> = sqlx::query_scalar(
        "SELECT status FROM dag_execution WHERE session_id = ? ORDER BY created_at",
    )
    .bind(SHUTDOWN_SESSION_ID)
    .fetch_all(&reopened_pool)
    .await
    .expect("shutdown tracer DAG statuses should load");
    assert_eq!(dag_statuses, vec!["COMPLETED"]);
    let job_statuses: Vec<String> =
        sqlx::query_scalar("SELECT status FROM job WHERE session_id = ? ORDER BY created_at")
            .bind(SHUTDOWN_SESSION_ID)
            .fetch_all(&reopened_pool)
            .await
            .expect("shutdown tracer job statuses should load");
    assert_eq!(
        job_statuses,
        vec!["COMPLETED"],
        "the configured paired-segment job must execute exactly once"
    );

    let active_session = session_repo
        .get_active_session_for_streamer(SHUTDOWN_STREAMER_ID)
        .await
        .expect("active shutdown tracer session should load")
        .expect("shutdown should preserve the active session");
    assert_eq!(active_session.id, SHUTDOWN_SESSION_ID);
    assert!(active_session.end_time.is_none());

    let resumed_output_dir = output_dir.to_string_lossy().into_owned();
    let updated = sqlx::query(
        "UPDATE global_config \
         SET record_danmu = FALSE, default_download_engine = 'ffmpeg', output_folder = ? \
         WHERE id = 'global-configuration'",
    )
    .bind(&resumed_output_dir)
    .execute(&reopened_write_pool)
    .await
    .expect("resume probe config should update");
    assert_eq!(updated.rows_affected(), 1);

    let resumed_container = super::ServiceContainer::with_full_config(
        reopened_pool.clone(),
        reopened_write_pool.clone(),
        super::ServiceContainerConfig {
            cache_ttl: Duration::from_secs(60),
            event_capacity: 8,
            download_config: DownloadManagerConfig::default(),
            pipeline_config: crate::pipeline::PipelineManagerConfig::default(),
            api_config: crate::api::server::ApiServerConfig::default(),
        },
    )
    .await
    .expect("resumed service container should initialize");
    resumed_container
        .streamer_manager
        .hydrate()
        .await
        .expect("resumed streamer metadata should hydrate");
    resumed_container.setup_download_event_subscriptions();
    resumed_container.setup_session_lifecycle_subscriptions();

    let resumed_segment_path = output_dir.join("segment-1.flv");
    let resumed_started = Arc::new(Notify::new());
    resumed_container
        .download_manager
        .register_engine(Arc::new(ShutdownFlushEngine {
            segment_path: resumed_segment_path,
            started: resumed_started.clone(),
        }));
    let mut resumed_events = resumed_container.download_manager.subscribe();
    let resume_streams = vec![
        platforms_parser::media::StreamInfo::builder(
            "https://example.invalid/resumed.flv",
            platforms_parser::media::StreamFormat::Flv,
            platforms_parser::media::formats::MediaFormat::Flv,
        )
        .build(),
    ];
    let live_args = |now| crate::session::LiveDetectedArgs {
        streamer_id: SHUTDOWN_STREAMER_ID,
        streamer_name: "Shutdown tracer",
        streamer_url: "https://example.com/shutdown-tracer",
        current_avatar: None,
        new_avatar: None,
        title: "Resumed shutdown tracer",
        category: None,
        streams: &resume_streams,
        media_headers: None,
        media_extras: None,
        now,
    };

    let hydrated = resumed_container
        .session_lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .expect("active persisted session should hydrate into the lifecycle");
    assert_eq!(hydrated.session_id(), SHUTDOWN_SESSION_ID);
    resumed_container
        .session_lifecycle
        .on_download_terminal(&DownloadTerminalEvent::Completed {
            download_id: "pre-restart-download".to_string(),
            streamer_id: SHUTDOWN_STREAMER_ID.to_string(),
            streamer_name: "Shutdown tracer".to_string(),
            session_id: SHUTDOWN_SESSION_ID.to_string(),
            total_bytes: expected_size as u64,
            total_duration_secs: 1.0,
            total_segments: 1,
            file_path: Some(segment_path_string.clone()),
            engine_signal: EngineEndSignal::CleanDisconnect,
            stop_cause: None,
        })
        .await
        .expect("clean disconnect should enter hysteresis");
    let resumed = resumed_container
        .session_lifecycle
        .on_live_detected(live_args(Utc::now()))
        .await
        .expect("hysteresis resume should publish the production restart payload");
    assert_eq!(resumed.session_id(), SHUTDOWN_SESSION_ID);
    tokio::time::timeout(Duration::from_secs(5), resumed_started.notified())
        .await
        .expect("production resume pipeline should open its segment");
    let resumed_index = loop {
        let event = tokio::time::timeout(Duration::from_secs(5), resumed_events.recv())
            .await
            .expect("resumed segment start should arrive")
            .expect("resumed event channel should remain open");
        if let DownloadManagerEvent::Progress(DownloadProgressEvent::SegmentStarted {
            segment_index,
            ..
        }) = event
        {
            break segment_index;
        }
    };
    assert_eq!(resumed_index, 1);
    resumed_container
        .shutdown_with_grace_period(Duration::from_secs(10))
        .await
        .expect("resumed service container should shut down cleanly");
    assert!(reopened_pool.is_closed());
    assert!(reopened_write_pool.is_closed());
}

/// The phased drain waits for containment, so a task that ignores the
/// cancellation token holds `shutdown_with_grace_period` open forever.
/// `shutdown_with_hard_cap` must stop waiting, abort the task, and report
/// what it aborted instead of leaving an embedder wedged.
#[tokio::test]
async fn hard_cap_aborts_supervised_work_that_never_settles() {
    let pool = migrated_test_pool().await;
    let container = super::ServiceContainer::with_config(
        pool.clone(),
        pool.clone(),
        Duration::from_secs(60),
        8,
    )
    .await
    .expect("service container should initialize");

    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    assert!(container.task_supervisor.spawn("wedged", async move {
        let _ = started_tx.send(());
        std::future::pending::<()>().await;
    }));
    started_rx.await.expect("wedged task should start");

    let hard_cap = Duration::from_millis(200);
    let started_at = std::time::Instant::now();
    let error = container
        .shutdown_with_hard_cap(Duration::from_millis(50), hard_cap)
        .await
        .expect_err("the wedged task must keep the drain from completing");
    let elapsed = started_at.elapsed();

    assert!(
        elapsed < hard_cap + Duration::from_millis(250),
        "aborting must stay inside the hard cap (allowing scheduler jitter): {elapsed:?}"
    );
    let message = error.to_string();
    assert!(
        message.contains("force deadline") && message.contains("1 background task(s)"),
        "the error must name the work that was aborted: {message}"
    );
    // Producer quiescence was never proven, so the pools stay open exactly
    // as in `shutdown_with_grace_period`; the caller exits the process.
    assert!(!pool.is_closed());
}

#[tokio::test]
async fn hard_cap_shutdown_under_the_cap_closes_pools() {
    let pool = migrated_test_pool().await;
    let container = super::ServiceContainer::with_config(
        pool.clone(),
        pool.clone(),
        Duration::from_secs(60),
        8,
    )
    .await
    .expect("service container should initialize");
    container.setup_download_event_subscriptions();
    container.setup_session_lifecycle_subscriptions();

    container
        .shutdown_with_hard_cap(Duration::from_millis(500), Duration::from_secs(5))
        .await
        .expect("a quiescent container should shut down within the cap");

    assert!(pool.is_closed());
}

#[tokio::test]
async fn broadcast_lag_is_recoverable_and_receiver_remains_usable() {
    let (sender, mut receiver) = tokio::sync::broadcast::channel(1);
    assert!(sender.send(1).is_ok());
    assert!(sender.send(2).is_ok());

    let error = receiver.recv().await.expect_err("receiver should lag");
    assert!(broadcast_error_is_recoverable("test", error));
    assert_eq!(receiver.recv().await, Ok(2));
}

#[tokio::test]
async fn closed_broadcast_channel_is_terminal() {
    let (sender, mut receiver) = tokio::sync::broadcast::channel::<u8>(1);
    drop(sender);

    let error = receiver.recv().await.expect_err("channel should be closed");
    assert!(!broadcast_error_is_recoverable("test", error));
}

#[tokio::test]
async fn full_config_wires_credential_notifications() {
    let pool = migrated_test_pool().await;

    let container = super::ServiceContainer::with_full_config(
        pool.clone(),
        pool,
        super::ServiceContainerConfig {
            cache_ttl: std::time::Duration::from_secs(60),
            event_capacity: 8,
            download_config: crate::downloader::DownloadManagerConfig::default(),
            pipeline_config: crate::pipeline::PipelineManagerConfig::default(),
            api_config: crate::api::server::ApiServerConfig::default(),
        },
    )
    .await
    .expect("full service container should initialize");

    assert!(container.credential_service.has_notification_service());
    container.cancellation_token().cancel();
}

#[tokio::test]
async fn standard_config_uses_the_unified_build_path() {
    let pool = migrated_test_pool().await;
    let container = super::ServiceContainer::with_config(
        pool.clone(),
        pool,
        std::time::Duration::from_secs(60),
        8,
    )
    .await
    .expect("standard service container should initialize");

    assert!(container.credential_service.has_notification_service());
    container.cancellation_token().cancel();
}

#[test]
fn test_should_end_stream_on_danmu_stream_closed_defaults_true() {
    assert!(should_end_stream_on_danmu_stream_closed(None));
    assert!(should_end_stream_on_danmu_stream_closed(Some("{}")));
    assert!(should_end_stream_on_danmu_stream_closed(Some(
        "{invalid json"
    )));
}

#[test]
fn test_should_end_stream_on_danmu_stream_closed_honors_false() {
    assert!(!should_end_stream_on_danmu_stream_closed(Some(
        r#"{"end_stream_on_danmu_stream_closed":false}"#,
    )));
}

#[test]
fn test_recovery_progress_requires_strong_signal() {
    assert!(!should_record_recovery_from_progress(&DownloadProgress {
        bytes_downloaded: RECOVERY_PROGRESS_MIN_BYTES - 1,
        speed_bytes_per_sec: 1024,
        ..DownloadProgress::default()
    }));

    assert!(should_record_recovery_from_progress(&DownloadProgress {
        bytes_downloaded: RECOVERY_PROGRESS_MIN_BYTES,
        speed_bytes_per_sec: 1024,
        ..DownloadProgress::default()
    }));

    assert!(should_record_recovery_from_progress(&DownloadProgress {
        segments_completed: 1,
        ..DownloadProgress::default()
    }));
}

// ========== Output-root gate recovery hook filter ==========

/// The recovery hook filters streamers by a per-root prefix built from
/// `set_infra_blocked`'s `last_error` format. The prefix must include
/// the root path + a trailing space so a Degraded → Healthy transition
/// on one root only resets streamers blocked on that root: `/rec`
/// cannot match `/rec/huya` entries and vice versa.
#[test]
fn recovery_hook_prefix_discriminates_between_sibling_roots() {
    use crate::downloader::LAST_ERROR_GATE_PREFIX;
    use std::path::Path;

    let root_a = Path::new("/rec/huya");
    let root_b = Path::new("/rec/douyu");

    let marker_a = format!("{} {} ", LAST_ERROR_GATE_PREFIX, root_a.display());
    let marker_b = format!("{} {} ", LAST_ERROR_GATE_PREFIX, root_b.display());

    // Realistic `last_error` values as written by set_infra_blocked.
    let le_a_not_found = format!(
        "{} {} (not_found)",
        LAST_ERROR_GATE_PREFIX,
        root_a.display()
    );
    let le_a_storage = format!(
        "{} {} (storage_full)",
        LAST_ERROR_GATE_PREFIX,
        root_a.display()
    );
    let le_b_not_found = format!(
        "{} {} (not_found)",
        LAST_ERROR_GATE_PREFIX,
        root_b.display()
    );
    let le_unrelated = "connection refused".to_string();

    // Root A marker must match root A entries regardless of io_kind.
    assert!(le_a_not_found.starts_with(&marker_a));
    assert!(le_a_storage.starts_with(&marker_a));
    // Root A marker must NOT match root B entries.
    assert!(!le_b_not_found.starts_with(&marker_a));
    // Root B marker must match its own entries.
    assert!(le_b_not_found.starts_with(&marker_b));
    // Neither marker should match unrelated errors.
    assert!(!le_unrelated.starts_with(&marker_a));
    assert!(!le_unrelated.starts_with(&marker_b));
}

/// Even more important: a shorter root marker must not accidentally
/// match longer sibling roots that share its prefix. If the gate
/// ever gets two roots where one is a prefix of the other (e.g. a
/// user sets `RUST_SREC_OUTPUT_ROOTS=/rec` and `/rec/archive`), the
/// `/rec` recovery must NOT reset streamers blocked on `/rec/archive`.
/// The trailing space in the marker is what makes this safe.
#[test]
fn recovery_hook_prefix_is_safe_against_prefix_collisions() {
    use crate::downloader::LAST_ERROR_GATE_PREFIX;

    let short_marker = format!("{} {} ", LAST_ERROR_GATE_PREFIX, "/rec");
    let long_entry = format!("{} /rec/archive (not_found)", LAST_ERROR_GATE_PREFIX);

    // Without the trailing space, this would match. With it, it doesn't.
    assert!(!long_entry.starts_with(&short_marker));

    // Sanity: the long root's own marker matches.
    let long_marker = format!("{} {} ", LAST_ERROR_GATE_PREFIX, "/rec/archive");
    assert!(long_entry.starts_with(&long_marker));
}

// ========== static_root_prefix (startup probe config discovery) ==========

#[test]
fn static_root_prefix_typical_rust_srec_template() {
    // The default rust-srec template uses {platform}/{streamer}/%Y%m%d.
    // Everything after `/rec/` is dynamic so the prefix is `/rec/`.
    assert_eq!(
        super::static_root_prefix("/rec/{platform}/{streamer}/%Y%m%d"),
        Some("/rec/".to_string())
    );
}

#[test]
fn static_root_prefix_strftime_only() {
    // No `{...}` variables — only strftime placeholders. Prefix is
    // everything before the first `%`.
    assert_eq!(
        super::static_root_prefix("/rec/recordings/%Y-%m-%d"),
        Some("/rec/recordings/".to_string())
    );
}

#[test]
fn static_root_prefix_static_template_no_placeholders() {
    // Literal path. Whole string is the "prefix". Still trims to the
    // last slash to keep the result a complete directory path.
    assert_eq!(
        super::static_root_prefix("/app/output"),
        Some("/app/".to_string())
    );
    // If it already ends with a slash, preserve it.
    assert_eq!(
        super::static_root_prefix("/app/output/"),
        Some("/app/output/".to_string())
    );
}

#[test]
fn static_root_prefix_partial_directory_name_rejected() {
    // Template interpolates into the middle of a directory name
    // (`/recordings-{streamer}/...`). The prefix `/recordings-` is
    // not a complete directory — the last slash is at position 0, so
    // the result is just `/`, which we reject as too broad.
    assert_eq!(
        super::static_root_prefix("/recordings-{streamer}/files"),
        None
    );
}

#[test]
fn static_root_prefix_no_leading_slash_rejected() {
    // Relative template (no root `/`). Can't produce a probe key.
    assert_eq!(super::static_root_prefix("{streamer}/files"), None);
    assert_eq!(super::static_root_prefix("recordings/{streamer}"), None);
}

#[test]
fn static_root_prefix_empty_template() {
    assert_eq!(super::static_root_prefix(""), None);
}

#[test]
fn static_root_prefix_multi_level_static_prefix() {
    // Deep static prefix before the first placeholder.
    assert_eq!(
        super::static_root_prefix("/mnt/storage/recordings/{platform}/{streamer}"),
        Some("/mnt/storage/recordings/".to_string())
    );
}
