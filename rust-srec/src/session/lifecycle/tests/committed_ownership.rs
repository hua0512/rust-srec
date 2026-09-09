use super::*;

use crate::config::ConfigEventBroadcaster;
use crate::database::committed_writer::{CommitPhase, CommitTestGate, CommittedWriter};
use crate::session::transition::SessionTransitionReceiver;
use crate::streamer::{CommittedStreamerState, StreamerManager};

struct Fixture {
    pool: SqlitePool,
    lifecycle: Arc<SessionLifecycle>,
    writer: Arc<CommittedWriter>,
    manager: StreamerManager<SqlxStreamerRepository>,
    supervisor: Arc<TaskSupervisor>,
    observer: broadcast::Receiver<SessionTransition>,
    required: SessionTransitionReceiver,
}

async fn fixture(window: Duration, audit_gate: Option<Arc<AuditGate>>) -> Fixture {
    let pool = setup_pool().await;
    let supervisor = Arc::new(TaskSupervisor::new());
    let writer = Arc::new(CommittedWriter::new(pool.clone(), supervisor.clone()).unwrap());
    let broadcaster = ConfigEventBroadcaster::new();
    let state = Arc::new(CommittedStreamerState::new(
        writer.clone(),
        broadcaster.clone(),
    ));
    let repository = Arc::new(
        SqlxStreamerRepository::new(pool.clone(), pool.clone()).with_committed_state(state.clone()),
    );
    let manager = StreamerManager::new(repository, broadcaster);
    manager.hydrate().await.unwrap();
    let (required_tx, required) = crate::session::session_transition_channel();
    let events = SqlxSessionEventRepository::new(pool.clone(), pool.clone());
    let lifecycle = Arc::new(
        SessionLifecycle::with_config(
            Arc::new(SessionLifecycleRepository::new(pool.clone()).with_committed_state(state)),
            Arc::new(OfflineClassifier::new()),
            32,
            HysteresisConfig::from_window(window),
        )
        .with_required_transition_sender(required_tx)
        .with_event_repo(Arc::new(GatedEvents {
            events,
            gate: audit_gate,
        })),
    );
    let observer = lifecycle.subscribe();
    Fixture {
        pool,
        lifecycle,
        writer,
        manager,
        supervisor,
        observer,
        required,
    }
}

impl Fixture {
    async fn transitions(&mut self, kinds: &[&str], sid: &str) {
        for kind in kinds {
            let observed = tokio::time::timeout(Duration::from_secs(2), self.observer.recv())
                .await
                .unwrap()
                .unwrap();
            let required = tokio::time::timeout(Duration::from_secs(2), self.required.recv())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert_eq!(observed.kind_str(), *kind);
            assert_eq!(required.kind_str(), *kind);
            assert_eq!(observed.session_id(), sid);
            assert_eq!(required.session_id(), sid);
        }
        assert!(self.observer.try_recv().is_err());
        assert!(futures::poll!(Box::pin(self.required.recv())).is_pending());
    }

    async fn start(&mut self) -> String {
        let sid = self
            .lifecycle
            .on_live_detected(live_args(Utc::now()))
            .await
            .unwrap()
            .session_id()
            .to_string();
        self.transitions(&["started"], &sid).await;
        sid
    }

    async fn park(&mut self, sid: &str) {
        self.lifecycle
            .on_download_terminal(&make_terminal_completed_clean_disconnect(sid))
            .await
            .unwrap();
        self.transitions(&["ending"], sid).await;
    }

    async fn settled(&self) {
        drop(
            tokio::time::timeout(
                Duration::from_secs(2),
                self.lifecycle.lock_streamer(STREAMER_ID),
            )
            .await
            .unwrap(),
        );
    }

    async fn close(self) {
        let report = self
            .lifecycle
            .shutdown_until(tokio::time::Instant::now() + Duration::from_secs(2))
            .await;
        assert!(report.failures.is_empty(), "{:?}", report.failures);
        assert!(self.supervisor.shutdown(Duration::from_secs(2)).await);
    }
}

#[derive(Clone, Copy, Debug)]
enum Mutation {
    Live,
    Resume,
    Offline,
    Terminal,
    Disable,
    Schedule,
}

impl Mutation {
    async fn apply(self, lifecycle: Arc<SessionLifecycle>, sid: String) -> Result<()> {
        match self {
            Self::Live | Self::Resume => {
                lifecycle.on_live_detected(live_args(Utc::now())).await?;
            }
            Self::Offline => {
                lifecycle
                    .on_offline_detected(OfflineDetectedArgs {
                        streamer_id: STREAMER_ID,
                        streamer_name: "Test",
                        session_id: Some(&sid),
                        state_was_live: true,
                        clear_errors: true,
                        signal: None,
                        now: Utc::now(),
                    })
                    .await?;
            }
            Self::Terminal => {
                lifecycle
                    .on_download_terminal(&make_terminal_completed_hls_endlist(&sid))
                    .await?
            }
            Self::Disable => {
                lifecycle.end_for_disable(STREAMER_ID, "Test").await?;
            }
            Self::Schedule => {
                lifecycle
                    .end_for_out_of_schedule(STREAMER_ID, "Test", StreamerState::Live)
                    .await?;
            }
        }
        Ok(())
    }

    fn ended(self) -> bool {
        !matches!(self, Self::Live | Self::Resume)
    }
}

async fn reached(gate: &CommitTestGate) {
    tokio::time::timeout(Duration::from_secs(2), gate.started.notified())
        .await
        .unwrap();
}

#[tokio::test]
async fn public_lifecycle_mutations_finish_after_cancellation_at_each_commit_phase() {
    for phase in [CommitPhase::BeforeCommit, CommitPhase::AfterCommit] {
        for mutation in [
            Mutation::Live,
            Mutation::Resume,
            Mutation::Offline,
            Mutation::Terminal,
            Mutation::Disable,
            Mutation::Schedule,
        ] {
            let mut f = fixture(Duration::from_secs(120), None).await;
            let sid = if matches!(mutation, Mutation::Live) {
                String::new()
            } else {
                f.start().await
            };
            if matches!(mutation, Mutation::Resume) {
                f.park(&sid).await;
            }
            let gate = Arc::new(CommitTestGate::default());
            f.writer.set_commit_gate(phase, Some(gate.clone()));
            let caller = tokio::spawn(mutation.apply(f.lifecycle.clone(), sid.clone()));
            reached(&gate).await;
            caller.abort();
            assert!(caller.await.unwrap_err().is_cancelled());
            let mut successor = Box::pin(f.lifecycle.end_for_disable(STREAMER_ID, "Test"));
            assert!(futures::poll!(&mut successor).is_pending());
            assert!(futures::poll!(Box::pin(f.lifecycle.lock_streamer(STREAMER_ID))).is_pending());
            assert!(f.observer.try_recv().is_err());
            drop(successor);
            f.writer.set_commit_gate(phase, None);
            gate.release.notify_one();
            f.settled().await;

            let sessions: Vec<(String, Option<i64>)> =
                sqlx::query_as("SELECT id, end_time FROM live_sessions WHERE streamer_id = ?")
                    .bind(STREAMER_ID)
                    .fetch_all(&f.pool)
                    .await
                    .unwrap();
            assert_eq!(sessions.len(), 1, "{mutation:?}");
            let actual_sid = &sessions[0].0;
            if !sid.is_empty() {
                assert_eq!(actual_sid, &sid);
            }
            assert_eq!(sessions[0].1.is_some(), mutation.ended(), "{mutation:?}");
            let current = f
                .lifecycle
                .current_session_for_streamer(STREAMER_ID)
                .unwrap();
            assert_eq!(&current.0, actual_sid);
            assert_eq!(current.1.is_ended(), mutation.ended());
            assert_eq!(
                f.lifecycle.session_snapshot(actual_sid).unwrap().is_ended(),
                mutation.ended()
            );
            assert!(!f.lifecycle.hysteresis.contains_key(actual_sid));
            let expected_state = match mutation {
                Mutation::Offline => StreamerState::NotLive,
                Mutation::Schedule => StreamerState::OutOfSchedule,
                _ => StreamerState::Live,
            };
            assert_eq!(
                streamer_state(&f.pool, STREAMER_ID).await,
                expected_state.to_string()
            );
            assert_eq!(
                f.manager.get_streamer_snapshot(STREAMER_ID).unwrap().state,
                expected_state
            );
            let kinds = match mutation {
                Mutation::Live => vec!["started"],
                Mutation::Resume => vec!["resumed", "started"],
                _ => vec!["ended"],
            };
            f.transitions(&kinds, actual_sid).await;
            let events = read_events(&f.pool, actual_sid).await;
            assert_eq!(
                events
                    .iter()
                    .filter(|(kind, _)| kind == "session_ended")
                    .count(),
                usize::from(mutation.ended())
            );
            if matches!(mutation, Mutation::Resume) {
                assert_eq!(
                    events
                        .iter()
                        .filter(|(kind, _)| kind == "session_resumed")
                        .count(),
                    1
                );
                assert_eq!(
                    events
                        .iter()
                        .filter(|(kind, _)| kind == "session_started")
                        .count(),
                    2
                );
            }
            f.close().await;
        }
    }
}

#[tokio::test]
async fn cancellation_before_writer_admission_leaves_lifecycle_and_hysteresis_unchanged() {
    for mutation in [
        Mutation::Live,
        Mutation::Resume,
        Mutation::Offline,
        Mutation::Terminal,
        Mutation::Disable,
        Mutation::Schedule,
    ] {
        let mut f = fixture(Duration::from_secs(120), None).await;
        let sid = if matches!(mutation, Mutation::Live) {
            String::new()
        } else {
            f.start().await
        };
        if !sid.is_empty() {
            f.park(&sid).await;
        }
        let connection = f.pool.acquire().await.unwrap();
        let mut caller = Box::pin(mutation.apply(f.lifecycle.clone(), sid.clone()));
        assert!(futures::poll!(&mut caller).is_pending());
        assert!(futures::poll!(Box::pin(f.lifecycle.lock_streamer(STREAMER_ID))).is_pending());
        drop(caller);
        f.settled().await;
        drop(connection);
        assert!(f.observer.try_recv().is_err());
        assert!(futures::poll!(Box::pin(f.required.recv())).is_pending());
        let active: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM live_sessions WHERE end_time IS NULL")
                .fetch_one(&f.pool)
                .await
                .unwrap();
        assert_eq!(active, i64::from(!sid.is_empty()));
        if sid.is_empty() {
            assert!(
                f.lifecycle
                    .current_session_for_streamer(STREAMER_ID)
                    .is_none()
            );
        } else {
            assert!(f.lifecycle.session_snapshot(&sid).unwrap().is_hysteresis());
            assert!(!f.lifecycle.hysteresis.get(&sid).unwrap().is_cancelled());
            assert_eq!(read_events(&f.pool, &sid).await.len(), 2);
        }
        f.close().await;
    }
}

#[tokio::test]
async fn timer_commit_survives_forced_shutdown_without_extending_the_hard_deadline() {
    for phase in [CommitPhase::BeforeCommit, CommitPhase::AfterCommit] {
        let mut f = fixture(Duration::ZERO, None).await;
        let sid = f.start().await;
        let gate = Arc::new(CommitTestGate::default());
        f.writer.set_commit_gate(phase, Some(gate.clone()));
        f.park(&sid).await;
        reached(&gate).await;
        let mut successor = Box::pin(f.lifecycle.end_for_disable(STREAMER_ID, "Test"));
        assert!(futures::poll!(&mut successor).is_pending());
        drop(successor);
        let aborted = tokio::time::timeout(
            Duration::from_secs(1),
            f.lifecycle.abort_timers(tokio::time::Instant::now()),
        )
        .await
        .expect("hard shutdown must return with a supervised commit still pending");
        assert!(aborted > 0);
        assert!(futures::poll!(Box::pin(f.lifecycle.lock_streamer(STREAMER_ID))).is_pending());
        assert!(f.observer.try_recv().is_err());
        f.writer.set_commit_gate(phase, None);
        gate.release.notify_one();
        f.settled().await;
        assert!(f.lifecycle.session_snapshot(&sid).unwrap().is_ended());
        assert_eq!(
            f.lifecycle
                .current_session_for_streamer(STREAMER_ID)
                .unwrap()
                .0,
            sid
        );
        let end_time: Option<i64> =
            sqlx::query_scalar("SELECT end_time FROM live_sessions WHERE id = ?")
                .bind(&sid)
                .fetch_one(&f.pool)
                .await
                .unwrap();
        assert!(end_time.is_some());
        f.transitions(&["ended"], &sid).await;
        assert_eq!(
            read_events(&f.pool, &sid)
                .await
                .iter()
                .filter(|(kind, _)| kind == "session_ended")
                .count(),
            1
        );
        f.close().await;
    }
}

struct AuditGate {
    kind: &'static str,
    barrier: CommitTestGate,
}

struct GatedEvents {
    events: SqlxSessionEventRepository,
    gate: Option<Arc<AuditGate>>,
}

#[async_trait::async_trait]
impl SessionEventRepository for GatedEvents {
    async fn insert(&self, row: &SessionEventDbModel) -> Result<()> {
        if let Some(gate) = &self.gate
            && row.kind == gate.kind
        {
            gate.barrier.started.notify_one();
            gate.barrier.release.notified().await;
        }
        self.events.insert(row).await
    }
    async fn list_for_session(&self, sid: &str) -> Result<Vec<crate::session::SessionEvent>> {
        self.events.list_for_session(sid).await
    }
    async fn list_for_streamer(&self, id: &str) -> Result<Vec<crate::session::SessionEvent>> {
        self.events.list_for_streamer(id).await
    }
}

#[tokio::test]
async fn hysteresis_memory_publication_and_resume_audit_keep_their_completion_owner() {
    for resume in [false, true] {
        let gate = Arc::new(AuditGate {
            kind: if resume {
                "session_resumed"
            } else {
                "hysteresis_entered"
            },
            barrier: CommitTestGate::default(),
        });
        let mut f = fixture(Duration::from_secs(120), Some(gate.clone())).await;
        let sid = f.start().await;
        if resume {
            f.park(&sid).await;
        }
        let lifecycle = f.lifecycle.clone();
        let caller_sid = sid.clone();
        let caller = tokio::spawn(async move {
            if resume {
                lifecycle
                    .on_live_detected(live_args(Utc::now()))
                    .await
                    .map(|_| ())
            } else {
                lifecycle
                    .on_download_terminal(&make_terminal_completed_clean_disconnect(&caller_sid))
                    .await
            }
        });
        reached(&gate.barrier).await;
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        let mut successor = Box::pin(f.lifecycle.end_for_disable(STREAMER_ID, "Test"));
        assert!(futures::poll!(&mut successor).is_pending());
        // COMMIT has already released its lease: only the lifecycle stripe can
        // keep the successor behind the remaining audit and Started publication.
        drop(
            tokio::time::timeout(Duration::from_secs(1), f.pool.acquire())
                .await
                .unwrap()
                .unwrap(),
        );
        assert!(futures::poll!(Box::pin(f.lifecycle.lock_streamer(STREAMER_ID))).is_pending());
        drop(successor);
        gate.barrier.release.notify_one();
        f.settled().await;
        assert_eq!(
            f.lifecycle.session_snapshot(&sid).unwrap().is_hysteresis(),
            !resume
        );
        assert_eq!(
            f.lifecycle
                .current_session_for_streamer(STREAMER_ID)
                .unwrap()
                .0,
            sid
        );
        f.transitions(
            if resume {
                &["resumed", "started"]
            } else {
                &["ending"]
            },
            &sid,
        )
        .await;
        assert_eq!(
            read_events(&f.pool, &sid)
                .await
                .iter()
                .filter(|(kind, _)| kind == gate.kind)
                .count(),
            1
        );
        if !resume {
            assert_eq!(f.lifecycle.hysteresis_tasks.lock().len(), 1);
        }
        f.close().await;
    }
}

#[tokio::test]
async fn failed_hysteresis_end_keeps_its_original_handle_and_can_be_retried() {
    for mutation in [
        Mutation::Offline,
        Mutation::Terminal,
        Mutation::Disable,
        Mutation::Schedule,
    ] {
        let mut f = fixture(Duration::from_secs(120), None).await;
        let sid = f.start().await;
        f.park(&sid).await;
        let deadline = f.lifecycle.hysteresis.get(&sid).unwrap().deadline;
        sqlx::query("CREATE TRIGGER reject_session_end BEFORE UPDATE OF end_time ON live_sessions BEGIN SELECT RAISE(ABORT, 'test end failure'); END")
            .execute(&f.pool).await.unwrap();
        assert!(
            mutation
                .apply(f.lifecycle.clone(), sid.clone())
                .await
                .is_err()
        );
        assert!(f.lifecycle.session_snapshot(&sid).unwrap().is_hysteresis());
        let handle = f.lifecycle.hysteresis.get(&sid).unwrap();
        assert_eq!(handle.deadline, deadline);
        assert!(!handle.is_cancelled());
        drop(handle);
        assert!(f.observer.try_recv().is_err());
        sqlx::query("DROP TRIGGER reject_session_end")
            .execute(&f.pool)
            .await
            .unwrap();
        mutation
            .apply(f.lifecycle.clone(), sid.clone())
            .await
            .unwrap();
        assert!(f.lifecycle.session_snapshot(&sid).unwrap().is_ended());
        f.transitions(&["ended"], &sid).await;
        assert_eq!(
            read_events(&f.pool, &sid)
                .await
                .iter()
                .filter(|(kind, _)| kind == "session_ended")
                .count(),
            1
        );
        f.close().await;
    }
}

async fn timer_after_failed_commit() -> (Fixture, String, Arc<CommitTestGate>) {
    let mut f = fixture(Duration::ZERO, None).await;
    let sid = f.start().await;
    sqlx::query("CREATE TABLE timer_commit_fault (session_id TEXT REFERENCES live_sessions(id) DEFERRABLE INITIALLY DEFERRED)")
        .execute(&f.pool).await.unwrap();
    sqlx::query("CREATE TRIGGER fail_timer_commit AFTER UPDATE OF end_time ON live_sessions BEGIN INSERT INTO timer_commit_fault VALUES ('missing-session'); END")
        .execute(&f.pool).await.unwrap();
    let gate = Arc::new(CommitTestGate::default());
    f.writer
        .set_commit_gate(CommitPhase::BeforeCommit, Some(gate.clone()));
    f.park(&sid).await;
    reached(&gate).await;
    gate.release.notify_one();
    f.settled().await;

    assert!(f.lifecycle.session_snapshot(&sid).unwrap().is_hysteresis());
    assert!(!f.lifecycle.hysteresis.get(&sid).unwrap().is_cancelled());
    assert_eq!(f.lifecycle.hysteresis_tasks.lock().len(), 1);
    assert!(f.observer.try_recv().is_err());
    let end_time: Option<i64> =
        sqlx::query_scalar("SELECT end_time FROM live_sessions WHERE id = ?")
            .bind(&sid)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert!(
        end_time.is_none(),
        "failed COMMIT rolls back the session end"
    );
    assert_eq!(read_events(&f.pool, &sid).await.len(), 2);
    sqlx::query("DROP TRIGGER fail_timer_commit")
        .execute(&f.pool)
        .await
        .unwrap();
    (f, sid, gate)
}

#[tokio::test]
async fn failed_timer_commit_reuses_its_timer_and_retries_without_losing_hysteresis() {
    let (mut f, sid, gate) = timer_after_failed_commit().await;
    reached(&gate).await;
    f.writer.set_commit_gate(CommitPhase::BeforeCommit, None);
    gate.release.notify_one();
    f.settled().await;
    assert!(f.lifecycle.session_snapshot(&sid).unwrap().is_ended());
    assert!(!f.lifecycle.hysteresis.contains_key(&sid));
    f.transitions(&["ended"], &sid).await;
    assert_eq!(
        read_events(&f.pool, &sid)
            .await
            .iter()
            .filter(|(kind, _)| kind == "session_ended")
            .count(),
        1
    );
    f.close().await;
}

#[tokio::test]
async fn retrying_timer_stops_on_resume_shutdown_or_handle_replacement() {
    for action in ["resume", "shutdown", "replace"] {
        let (mut f, sid, _) = timer_after_failed_commit().await;
        f.writer.set_commit_gate(CommitPhase::BeforeCommit, None);
        match action {
            "resume" => {
                let resumed = f
                    .lifecycle
                    .on_live_detected(live_args(Utc::now()))
                    .await
                    .unwrap();
                assert_eq!(resumed.session_id(), sid);
                f.transitions(&["resumed", "started"], &sid).await;
            }
            "shutdown" => {
                let report = tokio::time::timeout(
                    Duration::from_secs(1),
                    f.lifecycle
                        .shutdown_until(tokio::time::Instant::now() + Duration::from_secs(1)),
                )
                .await
                .unwrap();
                assert!(report.failures.is_empty());
            }
            "replace" => {
                // A replacement can deliberately retain the same session ID;
                // matching only that ID must not let the old timer end it.
                f.lifecycle
                    .hysteresis
                    .insert(sid.clone(), HysteresisHandle::new(Duration::from_secs(120)));
            }
            _ => unreachable!(),
        }
        let mut timers = DrainedTasks::take_from(&f.lifecycle.hysteresis_tasks);
        tokio::time::timeout(Duration::from_secs(2), async {
            while let Some(result) = timers.join_next().await {
                result.unwrap();
            }
        })
        .await
        .expect("retry wait must stop without abandoning its timer task");
        drop(timers);
        assert!(f.observer.try_recv().is_err());
        assert!(futures::poll!(Box::pin(f.required.recv())).is_pending());
        let end_time: Option<i64> =
            sqlx::query_scalar("SELECT end_time FROM live_sessions WHERE id = ?")
                .bind(&sid)
                .fetch_one(&f.pool)
                .await
                .unwrap();
        assert!(end_time.is_none());
        assert!(f.lifecycle.is_session_active(&sid));
        assert_eq!(
            read_events(&f.pool, &sid)
                .await
                .iter()
                .filter(|(kind, _)| kind == "session_ended")
                .count(),
            0
        );
        f.close().await;
    }
}
