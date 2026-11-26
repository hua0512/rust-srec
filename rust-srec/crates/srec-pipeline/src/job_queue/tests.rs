use std::time::Duration;

use super::*;
use crate::JobPhase;
use crate::PipelineResult as Result;
use crate::processors::UploadItemStatus;
use crate::progress::ProgressKind;
use crate::upload_events::{UploadStatusEvent, UploadTerminalStatus};
use srec_core::CoreError;
use srec_sqlite::models::{Pagination, UploadRecordDbModel};

/// The manifest travels in the `state` column with the placeholder metadata
/// and comes back through `db_model_to_job`; rows written before the column
/// existed, or with a corrupt value, yield no manifest.
#[test]
fn job_state_round_trips_the_manifest() {
    use crate::manifest::{ManifestScope, ManifestSegment, PipelineInputManifest};
    let manifest = Arc::new(PipelineInputManifest::new(
        "session",
        "streamer",
        ManifestScope::Session,
        vec![ManifestSegment {
            segment_index: 0,
            video: vec!["/rec/0.mp4".to_string()],
            danmu: vec!["/rec/0.xml".to_string()],
        }],
    ));
    let mut job = Job::new("remux", vec!["/rec/0.mp4".to_string()], vec![], "s", "sess");
    job.manifest = Some(manifest.clone());

    let state = job_state_json(&job);
    let parsed = parse_job_state(&state);
    assert_eq!(parsed.manifest.as_deref(), Some(manifest.as_ref()));
    assert!(
        !parsed.has_any(),
        "a manifest alone is not placeholder metadata"
    );
    assert_eq!(
        db_model_to_job(&job_to_db_model(&job)).manifest,
        job.manifest
    );
    let mut named = job.clone();
    named.streamer_name = Some("StreamerName".to_string());
    named.session_title = Some("SessionTitle".to_string());
    named.platform = Some("Twitch".to_string());
    let restored = db_model_to_job(&job_to_db_model(&named));
    assert_eq!(restored.manifest, job.manifest);
    assert_eq!(restored.platform.as_deref(), Some("Twitch"));
    assert_eq!(restored.streamer_name.as_deref(), Some("StreamerName"));
    assert_eq!(restored.session_title.as_deref(), Some("SessionTitle"));

    assert!(parse_job_state("{}").manifest.is_none());
    assert!(
        parse_job_state(r#"{"streamer_name":"S","manifest":null}"#)
            .manifest
            .is_none()
    );
    assert!(
        parse_job_state(r#"{"manifest":{"version":"nope"}}"#)
            .manifest
            .is_none()
    );
    job.manifest = None;
    assert_eq!(job_state_json(&job), "{}");
}

#[tokio::test]
async fn terminal_cursor_cleanup_does_not_split_log_write_lock() {
    let pool = srec_sqlite::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    srec_sqlite::run_migrations(&pool).await.unwrap();
    let repo = Arc::new(crate::repositories::SqlxJobRepository::new(
        pool.clone(),
        pool.clone(),
    ));
    let queue = JobQueue::with_repository(JobQueueConfig::default(), repo.clone());
    let job_id = queue
        .enqueue(Job::new("remux", vec![], vec![], "streamer", "session"))
        .await
        .unwrap();
    let old: Vec<_> = (0..MAX_PERSISTED_LOG_ROWS_PER_JOB - 1)
        .map(|i| JobLogEntry::info(format!("old-{i}")))
        .collect();
    queue.append_log_entry(&job_id, &old).await.unwrap();

    // Hold the same permit as an in-flight insert. Polling each writer once proves
    // both remain blocked, even though terminal cleanup evicts the count between them.
    let guard = queue.log_persistence_lock(&job_id).lock().await;
    let first_batch = [JobLogEntry::info("first late batch")];
    let second_batch = [JobLogEntry::info("second late batch")];
    let first = queue.append_log_entry(&job_id, &first_batch);
    tokio::pin!(first);
    assert!(futures::poll!(first.as_mut()).is_pending());
    queue.finalize_cancelled_job(&job_id);
    assert!(!queue.persisted_log_cursor.contains_key(&job_id));
    let second = queue.append_log_entry(&job_id, &second_batch);
    tokio::pin!(second);
    assert!(futures::poll!(second.as_mut()).is_pending());
    drop(guard);
    let (first, second) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::join!(first, second)
    })
    .await
    .unwrap();
    first.unwrap();
    second.unwrap();
    let (logs, count) = repo
        .list_execution_logs(&job_id, &Pagination::new(5000, 0))
        .await
        .unwrap();
    assert_eq!(count, MAX_PERSISTED_LOG_ROWS_PER_JOB as u64);
    assert!(
        logs.iter()
            .any(|entry| entry.message.as_deref() == Some("first late batch"))
    );
    assert!(
        logs.iter()
            .any(|entry| entry.message.as_deref() == Some("second late batch"))
    );
    assert!(!queue.persisted_log_cursor.contains_key(&job_id));
    sqlx::query(
        "CREATE TRIGGER reject_late_log BEFORE INSERT ON job_execution_logs
         BEGIN SELECT RAISE(FAIL, 'injected late failure'); END",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert!(
        queue
            .append_log_entry(&job_id, &[JobLogEntry::info("failed late batch")])
            .await
            .is_err()
    );
    assert!(!queue.persisted_log_cursor.contains_key(&job_id));
}

/// resolve_job_metadata must write back-filled values to the job row's
/// state column so a later dequeue does not depend on the live_sessions
/// row still existing.
#[tokio::test]
async fn test_resolve_job_metadata_persists_backfilled_state() {
    let dir = tempfile::TempDir::new().unwrap();
    let db_path = dir.path().join("job_queue_test.db");
    let db_url = format!("sqlite:{}?mode=rwc", db_path.to_string_lossy());
    let pool = srec_sqlite::init_pool(&db_url).await.unwrap();
    srec_sqlite::run_migrations(&pool).await.unwrap();

    // Satisfy the live_sessions -> streamers -> platform_config FK chain.
    sqlx::query("INSERT INTO platform_config (id, platform_name) VALUES ('p1', 'test')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO streamers (id, name, url, platform_config_id, state) \
         VALUES ('streamer-1', 'Streamer', 'https://example.com/s1', 'p1', 'NOT_LIVE')",
    )
    .execute(&pool)
    .await
    .unwrap();
    let session_start_ms: i64 = 1_704_152_700_000;
    sqlx::query(
        "INSERT INTO live_sessions (id, streamer_id, start_time) \
         VALUES ('session-1', 'streamer-1', ?)",
    )
    .bind(session_start_ms)
    .execute(&pool)
    .await
    .unwrap();

    struct SqliteSessionMetaPort(Arc<srec_sqlite::repositories::session::SqlxSessionRepository>);

    #[async_trait::async_trait]
    impl crate::ports::PipelineSessionPort for SqliteSessionMetaPort {
        async fn session_has_thumbnail(&self, _: &str) -> crate::PipelineResult<bool> {
            Ok(false)
        }
        async fn record_thumbnail_output(
            &self,
            _: &str,
            _: &str,
            _: i64,
        ) -> crate::PipelineResult<bool> {
            Ok(false)
        }
        async fn mark_session_complete_dispatched(&self, _: &str) -> crate::PipelineResult<()> {
            Ok(())
        }
        async fn session_pipeline_settled(&self, _: &str) -> crate::PipelineResult<bool> {
            Ok(false)
        }
        async fn list_session_ids_for_retirement(
            &self,
            _: &str,
        ) -> crate::PipelineResult<Vec<String>> {
            Ok(Vec::new())
        }
        async fn list_active_sessions(
            &self,
            _: u32,
            _: u32,
        ) -> crate::PipelineResult<Vec<crate::ports::PipelineSessionSummary>> {
            Ok(Vec::new())
        }
        async fn list_ended_sessions_pending_pipeline_recovery(
            &self,
            _: u32,
            _: u32,
        ) -> crate::PipelineResult<Vec<crate::ports::PipelineSessionSummary>> {
            Ok(Vec::new())
        }
        async fn get_session_summary(
            &self,
            _: &str,
        ) -> crate::PipelineResult<Option<crate::ports::PipelineSessionSummary>> {
            Ok(None)
        }
        async fn list_session_segments(
            &self,
            _: &str,
            _: u32,
            _: u32,
        ) -> crate::PipelineResult<Vec<crate::ports::PipelineSessionSegment>> {
            Ok(Vec::new())
        }
        async fn list_session_danmu_outputs(&self, _: &str) -> crate::PipelineResult<Vec<String>> {
            Ok(Vec::new())
        }
        async fn get_session_meta(
            &self,
            session_id: &str,
        ) -> crate::PipelineResult<Option<crate::ports::PipelineSessionMeta>> {
            use srec_sqlite::repositories::SessionRepository as _;
            match self.0.get_session(session_id).await {
                Ok(session) => Ok(Some(crate::ports::PipelineSessionMeta {
                    title: None,
                    started_at: srec_sqlite::time::ms_to_datetime(session.start_time),
                })),
                Err(srec_sqlite::DbError::Core(CoreError::NotFound { .. })) => Ok(None),
                Err(err) => Err(err.into()),
            }
        }
        async fn get_streamer_meta(
            &self,
            _: &str,
        ) -> crate::PipelineResult<Option<crate::ports::PipelineStreamerMeta>> {
            Ok(None)
        }
    }

    let job_repo: Arc<dyn JobRepository> = Arc::new(
        crate::repositories::job::SqlxJobRepository::new(pool.clone(), pool.clone()),
    );
    let queue = JobQueue::with_repository(JobQueueConfig::default(), job_repo.clone());
    queue.set_session_port(Arc::new(SqliteSessionMetaPort(Arc::new(
        srec_sqlite::repositories::session::SqlxSessionRepository::new(pool.clone(), pool.clone()),
    ))));

    let mut job = Job::new(
        "rclone",
        vec!["/input.flv".to_string()],
        vec![],
        "streamer-1",
        "session-1",
    );
    let manifest = Arc::new(crate::manifest::PipelineInputManifest::new(
        "session-1",
        "streamer-1",
        crate::manifest::ManifestScope::Session,
        Vec::new(),
    ));
    job.manifest = Some(manifest.clone());
    queue.enqueue(job.clone()).await.unwrap();
    assert!(
        parse_job_state(&job_repo.get_job(&job.id).await.unwrap().state)
            .session_start
            .is_none()
    );

    queue.resolve_job_metadata(&mut job).await;
    assert_eq!(
        job.session_start.map(|dt| dt.as_millisecond()),
        Some(session_start_ms)
    );

    // The resolved value must now be in the DB row, not just in memory, and
    // rewriting the state must keep the manifest that was stored with the job.
    let db_job = job_repo.get_job(&job.id).await.unwrap();
    let state: serde_json::Value = serde_json::from_str(&db_job.state).unwrap();
    assert_eq!(
        state.get("session_start_ms").and_then(|v| v.as_i64()),
        Some(session_start_ms)
    );
    assert_eq!(
        parse_job_state(&db_job.state).manifest.as_deref(),
        Some(manifest.as_ref())
    );
}

#[test]
fn test_upload_kind_for_job_type() {
    // Bare processor names and their "<type>_<preset>" DAG-step variants.
    assert_eq!(upload_kind_for_job_type("rclone"), Some("rclone"));
    assert_eq!(upload_kind_for_job_type("RCLONE"), Some("rclone"));
    assert_eq!(upload_kind_for_job_type("rclone_default"), Some("rclone"));
    assert_eq!(upload_kind_for_job_type("upload"), Some("rclone"));
    assert_eq!(upload_kind_for_job_type("upload_gdrive"), Some("rclone"));
    assert_eq!(upload_kind_for_job_type("baidupcs"), Some("baidupcs"));
    assert_eq!(upload_kind_for_job_type("BaiduPCS"), Some("baidupcs"));
    assert_eq!(
        upload_kind_for_job_type("baidupcs_default"),
        Some("baidupcs")
    );
    assert_eq!(upload_kind_for_job_type("remux"), None);
    assert_eq!(upload_kind_for_job_type("thumbnail"), None);
    // "uploader" would prefix-match "upload_" only with the underscore.
    assert_eq!(upload_kind_for_job_type("uploader"), None);
    assert_eq!(upload_kind_for_job_type("baidu"), None);
}

/// `complete()` must persist `JobResult.uploads` to upload_records for
/// upload job types and leave the table untouched for everything else.
#[tokio::test]
async fn test_complete_persists_upload_records_for_upload_jobs() {
    let pool = srec_sqlite::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    srec_sqlite::run_migrations(&pool).await.unwrap();

    let job_repo: Arc<dyn JobRepository> = Arc::new(
        crate::repositories::job::SqlxJobRepository::new(pool.clone(), pool.clone()),
    );
    let upload_repo = Arc::new(
        crate::repositories::upload_record::SqlxUploadRecordRepository::new(
            pool.clone(),
            pool.clone(),
        ),
    );
    let queue = JobQueue::with_repository(JobQueueConfig::default(), job_repo);
    queue.set_upload_repo(upload_repo.clone());

    let uploads = vec![UploadResultItem {
        local_path: "/videos/a.mp4".to_string(),
        remote_path: Some("remote:bucket/a.mp4".to_string()),
        size_bytes: Some(2048),
        status: UploadItemStatus::Completed,
        error: None,
    }];

    // Upload job type → rows persisted.
    let rclone_job = Job::new("rclone", vec!["/videos/a.mp4".to_string()], vec![], "", "");
    let rclone_id = queue.enqueue(rclone_job).await.unwrap();
    queue.dequeue(None).await.unwrap().unwrap();
    queue
        .complete(
            &rclone_id,
            JobResult {
                outputs: vec![],
                duration_secs: 1.0,
                metadata: None,
                uploads: uploads.clone(),
                logs: vec![],
                input_size_bytes: None,
                output_size_bytes: None,
            },
        )
        .await
        .unwrap();
    let rows = upload_repo.list_by_job(&rclone_id).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].uploader, "rclone");
    assert_eq!(rows[0].status, "COMPLETED");
    assert_eq!(rows[0].remote_path.as_deref(), Some("remote:bucket/a.mp4"));
    assert!(rows[0].completed_at.is_some());

    // Non-upload job type → uploads on the result are ignored.
    let remux_job = Job::new("remux", vec!["/videos/a.mp4".to_string()], vec![], "", "");
    let remux_id = queue.enqueue(remux_job).await.unwrap();
    queue.dequeue(None).await.unwrap().unwrap();
    queue
        .complete(
            &remux_id,
            JobResult {
                outputs: vec![],
                duration_secs: 1.0,
                metadata: None,
                uploads,
                logs: vec![],
                input_size_bytes: None,
                output_size_bytes: None,
            },
        )
        .await
        .unwrap();
    assert!(upload_repo.list_by_job(&remux_id).await.unwrap().is_empty());
}

/// A failed upload keeps the per-file results its processor reported and fails only the
/// inputs without one, with the job's error, and announces the outcome as retrying or
/// failed. A cancellation that wins the terminal transition leaves no failed rows.
#[tokio::test]
async fn failed_upload_records_and_terminal_events_follow_the_winning_failure() {
    let pool = srec_sqlite::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    srec_sqlite::run_migrations(&pool).await.unwrap();

    let job_repo: Arc<dyn JobRepository> = Arc::new(
        crate::repositories::job::SqlxJobRepository::new(pool.clone(), pool.clone()),
    );
    let upload_repo = Arc::new(
        crate::repositories::upload_record::SqlxUploadRecordRepository::new(
            pool.clone(),
            pool.clone(),
        ),
    );
    let queue = JobQueue::with_repository(JobQueueConfig::default(), job_repo);
    queue.set_upload_repo(upload_repo.clone());
    let broadcaster =
        UploadStatusBroadcaster::new(Arc::new(|_: &UploadStatusEvent| bytes::Bytes::new()));
    queue.set_upload_broadcaster(broadcaster.clone());
    let mut rx = broadcaster.subscribe();
    let mut terminal_of = async |job_id: &str| {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let envelope = rx.recv().await.expect("broadcast channel stays open");
                if let UploadStatusEvent::Terminal {
                    job_id: terminal_job,
                    files_succeeded,
                    files_failed,
                    files_skipped,
                    status,
                    error,
                    ..
                } = envelope.event.as_ref()
                    && terminal_job == job_id
                {
                    break (
                        (*files_succeeded, *files_failed, *files_skipped),
                        *status,
                        error.clone(),
                    );
                }
            }
        })
        .await
        .expect("a terminal upload event is emitted")
    };

    let failed_id = queue
        .enqueue(Job::new(
            "rclone",
            vec!["/videos/failed.mp4".to_string()],
            vec![],
            "streamer-1",
            "session-1",
        ))
        .await
        .unwrap();
    queue.dequeue(None).await.unwrap().unwrap();
    queue.fail(&failed_id, "connection reset").await.unwrap();

    let rows = upload_repo.list_by_job(&failed_id).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, "FAILED");
    assert_eq!(rows[0].error.as_deref(), Some("connection reset"));

    let batch_id = queue
        .enqueue(Job::new(
            "rclone",
            vec![
                "/rec/a.mp4".to_string(),
                "/rec/b.mp4".to_string(),
                "/rec/c.mp4".to_string(),
            ],
            vec![],
            "streamer",
            "session",
        ))
        .await
        .unwrap();
    queue.dequeue(None).await.unwrap().unwrap();
    let batch_error = "1 of 3 inputs failed: /rec/c.mp4: boom";
    queue
        .fail_with_step_info(
            &batch_id,
            batch_error,
            None,
            Some("rclone"),
            &[
                UploadResultItem {
                    local_path: "/rec/a.mp4".to_string(),
                    remote_path: Some("remote:/a.mp4".to_string()),
                    size_bytes: Some(1),
                    status: UploadItemStatus::Completed,
                    error: None,
                },
                UploadResultItem {
                    local_path: "/rec/b.mp4".to_string(),
                    remote_path: Some("remote:/b.mp4".to_string()),
                    size_bytes: Some(1),
                    status: UploadItemStatus::Skipped,
                    error: Some("exists".to_string()),
                },
            ],
        )
        .await
        .unwrap();
    let rows: Vec<_> = upload_repo
        .list_by_job(&batch_id)
        .await
        .unwrap()
        .into_iter()
        .map(|row| (row.local_path, row.status, row.remote_path, row.error))
        .collect();
    assert_eq!(
        rows,
        [
            (
                "/rec/a.mp4".to_string(),
                "COMPLETED".to_string(),
                Some("remote:/a.mp4".to_string()),
                None
            ),
            (
                "/rec/b.mp4".to_string(),
                "SKIPPED".to_string(),
                Some("remote:/b.mp4".to_string()),
                Some("exists".to_string())
            ),
            (
                "/rec/c.mp4".to_string(),
                "FAILED".to_string(),
                None,
                Some(batch_error.to_string())
            ),
        ]
    );
    assert_eq!(
        terminal_of(&batch_id).await,
        (
            (1, 1, 1),
            UploadTerminalStatus::Failed,
            Some(batch_error.to_string())
        )
    );

    // A failure with a scheduled retry is announced as retrying, the final one as failed.
    let retried_id = queue
        .enqueue(Job::new("rclone", vec!["/a.mp4".into()], vec![], "s1", ""))
        .await
        .unwrap();
    queue.dequeue(None).await.unwrap().unwrap();
    queue
        .fail_with_step_info(
            &retried_id,
            "rate limited",
            Some(Timestamp::now()),
            Some("rclone"),
            &[],
        )
        .await
        .unwrap();
    let (_, status, error) = terminal_of(&retried_id).await;
    assert_eq!((status, error), (UploadTerminalStatus::Retrying, None));
    queue.retry_job(&retried_id).await.unwrap();
    queue.dequeue(None).await.unwrap().unwrap();
    queue
        .fail_with_step_info(&retried_id, "exhausted", None, Some("rclone"), &[])
        .await
        .unwrap();
    let (_, status, error) = terminal_of(&retried_id).await;
    assert_eq!(
        (status, error),
        (UploadTerminalStatus::Failed, Some("exhausted".to_string()))
    );

    let cancelled_id = queue
        .enqueue(Job::new(
            "rclone",
            vec!["/videos/cancelled.mp4".to_string()],
            vec![],
            "streamer-1",
            "session-1",
        ))
        .await
        .unwrap();
    queue.dequeue(None).await.unwrap().unwrap();
    queue.cancel_job(&cancelled_id).await.unwrap();
    queue
        .fail(&cancelled_id, "late processor error")
        .await
        .unwrap();

    assert!(
        upload_repo
            .list_by_job(&cancelled_id)
            .await
            .unwrap()
            .is_empty(),
        "a cancellation that wins the transition must not leave failed upload rows"
    );
    queue.stop_progress_aggregator().await;
}

/// A snapshot only means anything while its job runs: it is dropped when
/// the job ends, and a report that reaches the aggregator after that does
/// not bring it back.
#[tokio::test]
async fn terminal_transitions_drop_progress_and_late_reports_stay_dropped() {
    async fn wait_until(mut done: impl FnMut() -> bool) {
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while !done() {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the aggregator handles the report");
    }

    let queue = JobQueue::with_repository(
        Default::default(),
        crate::test_support::job_repository().await,
    );

    let mut job_ids = Vec::new();
    for _ in 0..4 {
        let job = Job::new("remux", vec!["/in.mp4".to_string()], vec![], "", "");
        job_ids.push(queue.enqueue(job).await.unwrap());
    }
    for _ in &job_ids {
        queue.dequeue(None).await.unwrap().unwrap();
    }
    for job_id in &job_ids {
        queue
            .progress_reporter(job_id)
            .report(JobProgressSnapshot::new(ProgressKind::Ffmpeg));
    }
    wait_until(|| {
        job_ids
            .iter()
            .all(|job_id| queue.get_job_progress(job_id).is_some())
    })
    .await;

    let [completed, failed, cancelled, live] = job_ids.as_slice() else {
        unreachable!("four jobs were enqueued");
    };
    queue
        .complete(
            completed,
            JobResult {
                outputs: vec![],
                duration_secs: 1.0,
                metadata: None,
                uploads: vec![],
                logs: vec![],
                input_size_bytes: None,
                output_size_bytes: None,
            },
        )
        .await
        .unwrap();
    queue.fail(failed, "boom").await.unwrap();
    // A cancelled job keeps its tracking until its worker ends the attempt.
    let cancelled_attempt = queue.attempt_token(cancelled).unwrap();
    queue.cancel_job(cancelled).await.unwrap();
    queue.finalize_attempt(cancelled, &cancelled_attempt).await;
    let ended = [completed, failed, cancelled];
    for job_id in ended {
        assert!(queue.get_job_progress(job_id).is_none(), "{job_id}");
    }

    for job_id in ended {
        queue
            .progress_reporter(job_id)
            .report(JobProgressSnapshot::new(ProgressKind::Ffmpeg));
    }
    // The channel is FIFO: once this later report is cached, the late
    // ones before it have been handled.
    let mut probe = JobProgressSnapshot::new(ProgressKind::Ffmpeg);
    probe.percent = Some(50.0);
    queue.progress_reporter(live).report(probe);
    wait_until(|| {
        queue
            .get_job_progress(live)
            .is_some_and(|snapshot| snapshot.percent == Some(50.0))
    })
    .await;
    for job_id in ended {
        assert!(queue.get_job_progress(job_id).is_none(), "{job_id}");
    }
}

#[tokio::test]
async fn late_attempt_finalization_waits_for_successor_claim_and_keeps_its_tracking() {
    use std::time::Duration;

    use crate::repositories::{SqlxJobRepository, SqlxUploadRecordRepository};

    let pool = srec_sqlite::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    srec_sqlite::run_migrations(&pool).await.unwrap();
    let queue = JobQueue::with_repository(
        JobQueueConfig::default(),
        Arc::new(SqlxJobRepository::new(pool.clone(), pool.clone())),
    );
    queue.stop_progress_aggregator().await;
    let uploads = Arc::new(SqlxUploadRecordRepository::new(pool.clone(), pool.clone()));
    queue.set_upload_repo(uploads.clone());
    let broadcaster =
        UploadStatusBroadcaster::new(Arc::new(|_: &UploadStatusEvent| bytes::Bytes::new()));
    queue.set_upload_broadcaster(broadcaster.clone());
    let job_id = queue
        .enqueue(Job::new(
            "rclone",
            vec!["/input.mp4".to_string()],
            vec![],
            "",
            "",
        ))
        .await
        .unwrap();
    let (_, stale) = queue.dequeue(None).await.unwrap().unwrap();
    queue.cancel_job(&job_id).await.unwrap();
    queue.retry_job(&job_id).await.unwrap();

    // Pause a real claim before its cache/token publication. A late finalizer
    // must wait for that whole handoff rather than observe partially installed tracking.
    let connection = pool.acquire().await.unwrap();
    let mut dequeue = std::pin::pin!(queue.dequeue(None));
    assert!(futures::poll!(dequeue.as_mut()).is_pending());
    let mut finalization = std::pin::pin!(queue.finalize_attempt(&job_id, &stale));
    assert!(futures::poll!(finalization.as_mut()).is_pending());
    drop(connection);

    let successor = tokio::time::timeout(Duration::from_secs(3), dequeue)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(successor.0.id, job_id);
    let mut snapshot = JobProgressSnapshot::new(ProgressKind::Rclone);
    snapshot.percent = Some(42.0);
    queue.progress_cache.insert(job_id.clone(), snapshot);
    tokio::time::timeout(Duration::from_secs(3), finalization)
        .await
        .unwrap();
    assert!(!queue.attempt_token(&job_id).unwrap().is_cancelled());
    assert_eq!(queue.get_job_progress(&job_id).unwrap().percent, Some(42.0));

    let mut events = broadcaster.subscribe();
    queue.fail(&job_id, "successor failed").await.unwrap();
    let rows = uploads.list_by_job(&job_id).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].error.as_deref(), Some("successor failed"));
    assert!(matches!(
        events.try_recv().unwrap().event.as_ref(),
        UploadStatusEvent::Terminal {
            status: UploadTerminalStatus::Failed,
            files_failed: 1,
            ..
        }
    ));
}

/// A worker still holding an attempt that failed and was retried must not
/// settle the attempt the retry started.
#[tokio::test]
async fn superseded_attempt_cannot_settle_its_successor() {
    let queue = JobQueue::with_repository(
        Default::default(),
        crate::test_support::job_repository().await,
    );
    let job_id = queue
        .enqueue(Job::new("remux", vec!["/in.flv".into()], vec![], "", ""))
        .await
        .unwrap();
    let (_, stale) = queue.dequeue(None).await.unwrap().unwrap();
    queue.fail(&job_id, "timed out").await.unwrap();
    queue.retry_job(&job_id).await.unwrap();
    let (_, current) = queue.dequeue(None).await.unwrap().unwrap();

    let result = || JobResult {
        outputs: vec!["/out.mp4".into()],
        duration_secs: 1.0,
        metadata: None,
        uploads: vec![],
        logs: vec![],
        input_size_bytes: None,
        output_size_bytes: None,
    };
    assert!(
        !queue
            .complete_attempt(&job_id, &stale, result())
            .await
            .unwrap()
    );
    assert_eq!(
        queue.fail_attempt(&job_id, &stale, "late").await.unwrap(),
        JobFailureOutcome::Unchanged
    );
    assert_eq!(
        queue.get_job(&job_id).await.unwrap().unwrap().status,
        JobStatus::Processing
    );

    assert!(
        queue
            .complete_attempt(&job_id, &current, result())
            .await
            .unwrap()
    );
}

/// A failure whose retry is already due keeps the retry waiting until the
/// failed attempt's cleanup is done, so that cleanup cannot remove the
/// tracking of the attempt the retry starts.
#[tokio::test]
async fn due_retry_waits_for_the_failed_attempt_cleanup() {
    use std::time::Duration;

    use crate::repositories::SqlxJobRepository;

    /// Pauses the per-file upload write, which `fail_internal` makes after
    /// the failure is durable and before it cleans up the attempt.
    #[derive(Default)]
    struct PausedUploads {
        entered: Notify,
        release: Notify,
    }

    #[async_trait::async_trait]
    impl UploadRecordRepository for PausedUploads {
        async fn upsert_records(
            &self,
            _records: &[UploadRecordDbModel],
        ) -> srec_sqlite::DbResult<()> {
            self.entered.notify_one();
            self.release.notified().await;
            Ok(())
        }

        async fn list_by_job(
            &self,
            _job_id: &str,
        ) -> srec_sqlite::DbResult<Vec<UploadRecordDbModel>> {
            Ok(Vec::new())
        }

        async fn list_by_session_local_paths(
            &self,
            _output_keys: &[(String, String)],
        ) -> srec_sqlite::DbResult<Vec<UploadRecordDbModel>> {
            Ok(Vec::new())
        }
    }

    let pool = srec_sqlite::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    srec_sqlite::run_migrations(&pool).await.unwrap();
    let queue = JobQueue::with_repository(
        JobQueueConfig::default(),
        Arc::new(SqlxJobRepository::new(pool.clone(), pool.clone())),
    );
    queue.stop_progress_aggregator().await;
    let uploads = Arc::new(PausedUploads::default());
    queue.set_upload_repo(uploads.clone());
    let job_id = queue
        .enqueue(Job::new(
            "rclone",
            vec!["/input.mp4".to_string()],
            vec![],
            "",
            "",
        ))
        .await
        .unwrap();
    let (_, failed) = queue.dequeue(None).await.unwrap().unwrap();

    let mut failure = std::pin::pin!(queue.fail_with_step_info(
        &job_id,
        "boom",
        Some(Timestamp::now()),
        Some("rclone"),
        &[],
    ));
    tokio::select! {
        outcome = &mut failure => panic!("the upload write did not pause: {outcome:?}"),
        () = uploads.entered.notified() => {}
    }

    let mut successor = std::pin::pin!(async {
        queue.retry_job(&job_id).await.unwrap();
        queue.dequeue(None).await.unwrap().unwrap().1
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(200), &mut successor)
            .await
            .is_err(),
        "the retry started while the failed attempt was still cleaning up"
    );

    uploads.release.notify_one();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(3), failure)
            .await
            .unwrap()
            .unwrap(),
        JobFailureOutcome::Transitioned
    );
    let successor = tokio::time::timeout(Duration::from_secs(3), successor)
        .await
        .unwrap();
    assert_ne!(successor.id(), failed.id());
    assert_eq!(
        queue.attempt_token(&job_id).map(|token| token.id()),
        Some(successor.id())
    );
}

// ========================================================================
// Pipeline Recovery Tests
// ========================================================================

#[tokio::test]
async fn recovery_lists_files_staged_by_unfinished_attempts_only() {
    use crate::repositories::{SqlxStagedOutputRepository, StagedOutputRepository};

    let pool = srec_sqlite::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    srec_sqlite::run_migrations(&pool).await.unwrap();
    let repo = Arc::new(crate::repositories::SqlxJobRepository::new(
        pool.clone(),
        pool.clone(),
    ));
    let staged = Arc::new(SqlxStagedOutputRepository::new(pool.clone(), pool));
    let queue = JobQueue::with_repository(JobQueueConfig::default(), repo);
    queue.set_staged_output_repo(staged.clone());

    // One attempt finishes normally; the other never does, as in a crash.
    let finished = queue.staged_output_recorder("job-done");
    finished
        .tracker()
        .record(
            std::path::Path::new("/out/a.tmp-1.mp4"),
            std::path::Path::new("/out/a.mp4"),
        )
        .await;
    finished.finish().await;
    queue
        .staged_output_recorder("job-crashed")
        .tracker()
        .record(
            std::path::Path::new("/out/b.tmp-2.mp4"),
            std::path::Path::new("/out/b.mp4"),
        )
        .await;

    queue.recover_jobs().await.unwrap();

    let listed = staged.list_interrupted(None, 10).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].job_id, "job-crashed");
    assert_eq!(listed[0].path, "/out/b.tmp-2.mp4");
    assert_eq!(listed[0].output_path, "/out/b.mp4");
}

/// Silent uploads and long jobs are kept alive by repeating what the
/// client already knows: the latest progress, or the start event before
/// any progress exists. Other jobs, and cancelling ones, stay quiet.
#[tokio::test]
async fn heartbeat_reannounces_live_uploads_and_long_jobs() {
    let queue = JobQueue::with_repository(
        Default::default(),
        crate::test_support::job_repository().await,
    );
    let mut ids = Vec::new();
    for job_type in ["rclone", "baidupcs_main", "rclone", "remux", "remux"] {
        let job = Job::new(job_type, vec!["/in.flv".to_string()], vec![], "s", "");
        ids.push(queue.enqueue(job).await.unwrap());
        queue.dequeue(None).await.unwrap().unwrap();
    }
    let [with_progress, silent, cancelling, short, long] = ids.as_slice() else {
        unreachable!()
    };
    let mut snapshot = JobProgressSnapshot::new(ProgressKind::Rclone);
    snapshot.percent = Some(42.0);
    queue
        .progress_cache
        .insert(with_progress.clone(), snapshot.clone());
    for id in [short, long] {
        queue
            .progress_cache
            .insert(id.clone(), JobProgressSnapshot::new(ProgressKind::Ffmpeg));
    }
    queue.cancellation_tokens.get(cancelling).unwrap().cancel();
    let now = Timestamp::now();
    queue.jobs_cache.get_mut(long).unwrap().started_at = Some(now - LONG_JOB_AFTER);
    queue.long_job_tracking.get_mut(long).unwrap().announced = true;

    let events = heartbeat_events(
        &queue.jobs_cache,
        &queue.cancellation_tokens,
        &queue.long_job_tracking,
        &queue.progress_cache,
    );

    assert_eq!(events.len(), 3, "{events:?}");
    for event in &events {
        match event {
            UploadStatusEvent::Progress {
                job_id, snapshot, ..
            } => {
                assert_eq!(job_id, with_progress);
                assert_eq!(snapshot.percent, Some(42.0));
            }
            UploadStatusEvent::Started {
                job_id, uploader, ..
            } => {
                assert_eq!(job_id, silent);
                assert_eq!(*uploader, "baidupcs");
            }
            UploadStatusEvent::JobProgress { job_id, .. } => assert_eq!(job_id, long),
            other => panic!("unexpected heartbeat event {other:?}"),
        }
    }
}

/// Jobs other than uploads reach the socket only after
/// `JobQueueConfig::long_job_after` of processing: announced with the
/// progress they already have, then their progress, then their end, and
/// listed in the connect snapshot. Shorter jobs stay off it entirely.
#[tokio::test]
async fn long_jobs_reach_the_socket_after_the_long_job_delay() {
    // Far longer than the few database calls a short job makes here.
    const LONG_JOB_AFTER: std::time::Duration = std::time::Duration::from_millis(500);
    async fn next_event(
        rx: &mut tokio::sync::broadcast::Receiver<crate::UploadBroadcastEnvelope>,
    ) -> Arc<UploadStatusEvent> {
        // Well under HEARTBEAT_INTERVAL, so a heartbeat cannot stand in
        // for an event that has to come sooner.
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let envelope = rx.recv().await.expect("broadcast channel stays open");
                // The queued-uploads count is published independently.
                if !matches!(*envelope.event, UploadStatusEvent::Queue { .. }) {
                    return envelope.event;
                }
            }
        })
        .await
        .expect("an event is published")
    }

    let queue = JobQueue::with_repository(
        JobQueueConfig {
            long_job_after: LONG_JOB_AFTER,
            ..Default::default()
        },
        crate::test_support::job_repository().await,
    );
    let broadcaster =
        UploadStatusBroadcaster::new(Arc::new(|_: &UploadStatusEvent| bytes::Bytes::new()));
    queue.set_upload_broadcaster(broadcaster.clone());
    let mut rx = broadcaster.subscribe();
    let dequeued_at = tokio::time::Instant::now();
    let mut ids = Vec::new();
    for job_type in ["thumbnail", "remux"] {
        let job = Job::new(job_type, vec![], vec![], "s", "");
        ids.push(queue.enqueue(job).await.unwrap());
        queue.dequeue(None).await.unwrap().unwrap();
    }
    let [short, long] = ids.as_slice() else {
        unreachable!()
    };
    assert!(queue.list_active_jobs().await.unwrap().is_empty());
    // Cached before the announcement, with nothing reported after it, as
    // while FFmpeg finalizes an MP4.
    let mut finalizing = JobProgressSnapshot::new(ProgressKind::Ffmpeg);
    finalizing.phase = Some(JobPhase::Finalizing);
    queue.progress_cache.insert(long.clone(), finalizing);

    queue.fail(short, "boom").await.unwrap();
    let started_at_ms = match &*next_event(&mut rx).await {
        UploadStatusEvent::JobStarted {
            job_id,
            job_type,
            started_at_ms,
            ..
        } => {
            assert_eq!((job_id, job_type.as_str()), (long, "remux"));
            *started_at_ms
        }
        other => panic!("expected JobStarted, got {other:?}"),
    };
    assert!(dequeued_at.elapsed() >= LONG_JOB_AFTER);
    match &*next_event(&mut rx).await {
        UploadStatusEvent::JobProgress {
            job_id, snapshot, ..
        } => assert_eq!((job_id, snapshot.phase), (long, Some(JobPhase::Finalizing))),
        other => panic!("expected the cached JobProgress, got {other:?}"),
    }
    // With the announcement, not from the heartbeat.
    assert!(dequeued_at.elapsed() < HEARTBEAT_INTERVAL);
    let active = queue.list_active_jobs().await.unwrap();
    assert_eq!(
        active.iter().map(|job| &job.job_id).collect::<Vec<_>>(),
        [long]
    );

    let mut snapshot = JobProgressSnapshot::new(ProgressKind::Ffmpeg);
    snapshot.percent = Some(42.0);
    queue.progress_reporter(long).report(snapshot);
    match &*next_event(&mut rx).await {
        UploadStatusEvent::JobProgress {
            job_id, snapshot, ..
        } => assert_eq!((job_id, snapshot.percent), (long, Some(42.0))),
        other => panic!("expected JobProgress, got {other:?}"),
    }

    queue.fail(long, "boom").await.unwrap();
    match &*next_event(&mut rx).await {
        UploadStatusEvent::JobEnded {
            job_id,
            started_at_ms: ended_started_at_ms,
        } => assert_eq!((job_id, *ended_started_at_ms), (long, started_at_ms)),
        other => panic!("expected JobEnded, got {other:?}"),
    }
}

/// Live upload progress, rclone's and BaiduPCS's, is published while the
/// queued-uploads count waits for a database connection, and the count
/// follows once one is free.
#[tokio::test]
async fn progress_flows_while_the_queued_upload_count_waits_for_the_database() {
    use std::time::Duration;

    let pool = srec_sqlite::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    srec_sqlite::run_migrations(&pool).await.unwrap();
    // Taken before the aggregator first runs: its first queue poll is
    // immediate and has to wait for this connection.
    let connection = pool.acquire().await.unwrap();
    let queue = JobQueue::with_repository(
        JobQueueConfig::default(),
        Arc::new(crate::repositories::SqlxJobRepository::new(
            pool.clone(),
            pool.clone(),
        )),
    );
    let broadcaster =
        UploadStatusBroadcaster::new(Arc::new(|_: &UploadStatusEvent| bytes::Bytes::new()));
    queue.set_upload_broadcaster(broadcaster.clone());
    let mut rx = broadcaster.subscribe();
    // The aggregator publishes only for live jobs; a processing job owns a
    // cancellation token (created on dequeue), which is all the liveness check reads.
    for (job_id, kind) in [
        ("job-1", ProgressKind::Rclone),
        ("job-2", ProgressKind::BaiduPcs),
    ] {
        queue
            .cancellation_tokens
            .insert(job_id.to_string(), AttemptToken::default());
        let mut snapshot = JobProgressSnapshot::new(kind);
        snapshot.percent = Some(42.0);
        snapshot.bytes_done = Some(4200);
        queue.progress_reporter(job_id).report(snapshot);
    }

    // One flush publishes both, in either order.
    let mut published = Vec::new();
    for _ in 0..2 {
        let envelope = tokio::time::timeout(Duration::from_secs(3), rx.recv())
            .await
            .expect("progress is published while the count waits")
            .expect("broadcast channel stays open");
        match envelope.event.as_ref() {
            UploadStatusEvent::Progress {
                job_id, snapshot, ..
            } => published.push((job_id.clone(), snapshot.percent, snapshot.bytes_done)),
            other => panic!("expected Progress, got {other:?}"),
        }
    }
    published.sort_by(|left, right| left.0.cmp(&right.0));
    assert_eq!(
        published,
        [
            ("job-1".to_string(), Some(42.0), Some(4200)),
            ("job-2".to_string(), Some(42.0), Some(4200)),
        ]
    );

    drop(connection);
    let envelope = tokio::time::timeout(Duration::from_secs(3), rx.recv())
        .await
        .expect("the count is published once the database is free")
        .expect("broadcast channel stays open");
    assert!(matches!(
        envelope.event.as_ref(),
        UploadStatusEvent::Queue { pending: 0 }
    ));
}

#[tokio::test]
async fn cancel_job_and_cancel_pipeline_serialize_with_in_flight_dequeue() {
    use crate::{
        PipelineSessionMeta, PipelineSessionPort, PipelineSessionSegment, PipelineSessionSummary,
        PipelineStreamerMeta,
    };
    use async_trait::async_trait;

    struct BlockingSessionPort {
        paused: tokio::sync::Notify,
        release: tokio::sync::Notify,
    }

    #[async_trait]
    impl PipelineSessionPort for BlockingSessionPort {
        async fn session_has_thumbnail(&self, _session_id: &str) -> Result<bool> {
            unimplemented!()
        }
        async fn record_thumbnail_output(
            &self,
            _session_id: &str,
            _output_path: &str,
            _size_bytes: i64,
        ) -> Result<bool> {
            unimplemented!()
        }
        async fn mark_session_complete_dispatched(&self, _session_id: &str) -> Result<()> {
            unimplemented!()
        }
        async fn session_pipeline_settled(&self, _session_id: &str) -> Result<bool> {
            unimplemented!()
        }
        async fn list_session_ids_for_retirement(&self, _streamer_id: &str) -> Result<Vec<String>> {
            unimplemented!()
        }
        async fn list_active_sessions(
            &self,
            _limit: u32,
            _offset: u32,
        ) -> Result<Vec<PipelineSessionSummary>> {
            unimplemented!()
        }
        async fn list_ended_sessions_pending_pipeline_recovery(
            &self,
            _limit: u32,
            _offset: u32,
        ) -> Result<Vec<PipelineSessionSummary>> {
            unimplemented!()
        }
        async fn get_session_summary(
            &self,
            _session_id: &str,
        ) -> Result<Option<PipelineSessionSummary>> {
            unimplemented!()
        }
        async fn list_session_segments(
            &self,
            _session_id: &str,
            _limit: u32,
            _offset: u32,
        ) -> Result<Vec<PipelineSessionSegment>> {
            unimplemented!()
        }
        async fn list_session_danmu_outputs(&self, _session_id: &str) -> Result<Vec<String>> {
            unimplemented!()
        }
        async fn get_session_meta(&self, _session_id: &str) -> Result<Option<PipelineSessionMeta>> {
            unimplemented!()
        }
        async fn get_streamer_meta(
            &self,
            _streamer_id: &str,
        ) -> Result<Option<PipelineStreamerMeta>> {
            self.paused.notify_one();
            self.release.notified().await;
            Ok(None)
        }
    }

    for mode in ["cancel_job", "cancel_pipeline"] {
        let repo = crate::test_support::job_repository().await;
        let queue = Arc::new(JobQueue::with_repository(Default::default(), repo));
        let blocker = Arc::new(BlockingSessionPort {
            paused: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        });
        queue.set_session_port(blocker.clone());

        let mut job = Job::new("remux", vec!["/in.flv".into()], vec![], "streamer-1", "");
        job.pipeline_id = Some("pipe-1".to_string());
        let job_id = queue.enqueue(job).await.unwrap();

        let dequeue_queue = queue.clone();
        let dequeuing = tokio::spawn(async move { dequeue_queue.dequeue(None).await });
        tokio::time::timeout(Duration::from_secs(2), blocker.paused.notified())
            .await
            .unwrap();

        let cancel_queue = queue.clone();
        let cancel_id = job_id.clone();
        let mut cancelling = tokio::spawn(async move {
            if mode == "cancel_job" {
                cancel_queue.cancel_job(&cancel_id).await.map(|_| ())
            } else {
                cancel_queue.cancel_pipeline("pipe-1").await.map(|_| ())
            }
        });
        blocker.release.notify_one();
        let dequeued = tokio::time::timeout(Duration::from_secs(2), dequeuing)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(dequeued.is_some());
        tokio::time::timeout(Duration::from_secs(2), &mut cancelling)
            .await
            .unwrap()
            .unwrap()
            .unwrap();

        assert!(
            !queue.jobs_cache.contains_key(&job_id),
            "{mode} must not leave a stale Processing entry in jobs_cache"
        );
        let token = queue
            .attempt_token(&job_id)
            .expect("attempt token retained for cancelled in-flight worker");
        assert!(
            token.is_cancelled(),
            "{mode} must cancel the dequeued attempt token"
        );
        assert_eq!(
            queue.get_job(&job_id).await.unwrap().unwrap().status,
            JobStatus::Cancelled
        );
        queue.stop_progress_aggregator().await;
    }
}

#[tokio::test]
async fn retried_job_queue_wait_uses_reenqueue_timestamp_in_sqlite() {
    use crate::repositories::{JobRepository, SqlxJobRepository};

    let pool = srec_sqlite::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    srec_sqlite::run_migrations(&pool).await.unwrap();
    let repo = Arc::new(SqlxJobRepository::new(pool.clone(), pool.clone()));
    let queue = JobQueue::with_repository(JobQueueConfig::default(), repo.clone());

    let mut job = Job::new("rclone", vec!["/a.mp4".into()], vec![], "s1", "");
    job.has_step_retry = Some(true);
    let job_id = queue.enqueue(job).await.unwrap();

    // Backdate created_at by 1 hour so a regression falling back to created_at is unmistakable.
    let one_hour_ago_ms = srec_sqlite::time::now_ms() - 3_600_000;
    sqlx::query("UPDATE job SET created_at = ? WHERE id = ?")
        .bind(one_hour_ago_ms)
        .bind(&job_id)
        .execute(&pool)
        .await
        .unwrap();

    queue.dequeue(None).await.unwrap().unwrap();
    queue.fail(&job_id, "temporary error").await.unwrap();
    queue.retry_job(&job_id).await.unwrap();

    // Seed a known 12-second queue wait after re-enqueue so an implementation that
    // always reports 0 fails just as cleanly as one that reports 3600s.
    let twelve_secs_ago_ms = srec_sqlite::time::now_ms() - 12_000;
    sqlx::query("UPDATE job SET state = json_set(state, '$.enqueued_at_ms', ?) WHERE id = ?")
        .bind(twelve_secs_ago_ms)
        .bind(&job_id)
        .execute(&pool)
        .await
        .unwrap();

    let (retried, _attempt) = queue.dequeue(None).await.unwrap().unwrap();
    assert_eq!(retried.has_step_retry, Some(true));
    assert_eq!(retried.enqueued_at_ms, Some(twelve_secs_ago_ms));

    queue
        .complete(
            &job_id,
            JobResult {
                outputs: vec![],
                duration_secs: 1.0,
                metadata: None,
                uploads: vec![],
                logs: vec![],
                input_size_bytes: None,
                output_size_bytes: None,
            },
        )
        .await
        .unwrap();
    let stored = repo.get_job(&job_id).await.unwrap();
    let queue_wait = stored.queue_wait_secs.expect("queue_wait_secs recorded");
    assert!(
        (10.0..60.0).contains(&queue_wait),
        "queue_wait_secs should reflect the ~12s re-enqueue wait: {queue_wait}"
    );

    // Both startup recovery and retry reset normalize non-object JSON state (`null`)
    // so `enqueued_at_ms` is recorded cleanly.
    let null_state_job_id = queue
        .enqueue(Job::new("rclone", vec!["/b.mp4".into()], vec![], "s1", ""))
        .await
        .unwrap();
    queue.dequeue(None).await.unwrap().unwrap();
    sqlx::query("UPDATE job SET created_at = ?, state = 'null' WHERE id = ?")
        .bind(one_hour_ago_ms)
        .bind(&null_state_job_id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(repo.reset_processing_jobs().await.unwrap(), 1);
    let recovered = repo.get_job(&null_state_job_id).await.unwrap();
    assert!(
        super::metadata::parse_job_enqueued_at_ms(&recovered.state)
            .is_some_and(|ms| ms > one_hour_ago_ms),
        "startup reset must normalize non-object JSON state and set enqueued_at_ms"
    );

    queue.dequeue(None).await.unwrap().unwrap();
    queue.fail(&null_state_job_id, "temporary").await.unwrap();
    sqlx::query("UPDATE job SET state = 'null' WHERE id = ?")
        .bind(&null_state_job_id)
        .execute(&pool)
        .await
        .unwrap();
    queue.retry_job(&null_state_job_id).await.unwrap();
    let (normalized, _) = queue.dequeue(None).await.unwrap().unwrap();
    assert!(
        normalized
            .enqueued_at_ms
            .is_some_and(|ms| ms > one_hour_ago_ms),
        "retry reset must normalize non-object JSON state and set enqueued_at_ms"
    );
    queue.stop_progress_aggregator().await;
}
