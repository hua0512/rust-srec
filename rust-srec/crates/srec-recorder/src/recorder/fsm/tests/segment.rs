use std::time::Duration;

use crate::danmu::DanmuControlEvent;
use crate::downloader::{DownloadStopCause, EngineEndSignal};
use crate::recorder::fsm::state::{
    DanmuSessionState, FsmEffect, FsmInput, MonitorPhase, SessionSlot,
};
use crate::recorder::ports::{DanmuStopResult, DbCommitOp, DbCommitOutcome, PipelineNotification};
use crate::session::{OfflineSignal, TerminalCause};
use srec_core::domain::StreamerState;

use super::{
    TestClock, base_config, completed_event, find_db_commit, open_running_session,
    session_ended_outcome,
};

#[test]
fn discarded_segment_danmu_is_deleted_and_holds_the_stop_until_it_lands() {
    let clock = TestClock::new();
    let mut cfg = base_config();
    cfg.record_danmu = true;
    cfg.min_segment_size_bytes = 1_000_000;
    let (mut fsm, attempt_id) = open_running_session(&clock, cfg, "sess-seg", None);
    fsm.step(
        clock.ctx(),
        FsmInput::DanmuCollectionStarted {
            session_id: "sess-seg".into(),
        },
    );

    let effects = fsm.step(
        clock.ctx(),
        FsmInput::SegmentCompleted {
            attempt_id,
            session_id: "sess-seg".into(),
            download_id: "dl-1".into(),
            segment_index: 0,
            segment_path: "/rec/seg0.flv".into(),
            started_at: Some(clock.wall),
            completed_at: clock.wall,
            duration_secs: 2.0,
            size_bytes: 50_000,
            effective_size_bytes: 50_000,
            split_reason_code: None,
            split_reason_details_json: None,
            split_by_format_change: false,
        },
    );
    assert!(effects.iter().any(|e| matches!(
        e,
        FsmEffect::CloseDanmuSegment(spec) if spec.segment_index == 0 && spec.discard
    )));
    assert!(effects.iter().any(|e| matches!(
        e,
        FsmEffect::DeleteDiscardedFiles {
            video_path: Some(v),
            danmu_path: Some(d),
        } if v == "/rec/seg0.flv" && d == "/rec/seg0.xml"
    )));

    let (end_op, _) = find_db_commit(&fsm.step(
        clock.ctx(),
        FsmInput::DownloadEnded {
            attempt_id,
            event: completed_event("sess-seg", EngineEndSignal::HlsEndlist),
        },
    ))
    .unwrap();
    fsm.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id: end_op,
            result: Ok(session_ended_outcome(Some("sess-seg"), clock.wall, true)),
        },
    );
    assert!(!fsm.is_physically_quiescent());

    let effects = fsm.step(
        clock.ctx(),
        FsmInput::DanmuSegmentCompleted {
            session_id: "sess-seg".into(),
            segment_index: 0,
            output_path: "/rec/seg0.xml".into(),
            message_count: 10,
            size_bytes: 500,
        },
    );
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e, FsmEffect::PersistDanmuSegment(_)))
    );
    assert!(effects.iter().any(|e| matches!(
        e,
        FsmEffect::DeleteDiscardedFiles {
            video_path: None,
            danmu_path: Some(d),
        } if d == "/rec/seg0.xml"
    )));

    let effects = fsm.step(
        clock.ctx(),
        FsmInput::DanmuCollectionStopped {
            session_id: "sess-seg".into(),
            total_count: 10,
            result: DanmuStopResult::Terminated,
        },
    );
    assert!(effects.iter().any(|e| matches!(
        e,
        FsmEffect::NotifyPipeline(PipelineNotification::DanmuCollectionStopped {
            session_id,
            total_count: 10,
        }) if session_id == "sess-seg"
    )));
    assert!(fsm.is_physically_quiescent());
}

#[test]
fn reverse_danmu_cleanup_and_trailing_stale_attempt_segment_persistence() {
    let clock = TestClock::new();
    let mut cfg = base_config();
    cfg.record_danmu = true;
    cfg.min_segment_size_bytes = 500_000;
    let (mut fsm, attempt_id) = open_running_session(&clock, cfg, "sess-rev", None);
    fsm.step(
        clock.ctx(),
        FsmInput::DanmuCollectionStarted {
            session_id: "sess-rev".into(),
        },
    );

    let effects = fsm.step(
        clock.ctx(),
        FsmInput::DanmuSegmentCompleted {
            session_id: "sess-rev".into(),
            segment_index: 0,
            output_path: "/rec/seg0.xml".into(),
            message_count: 25,
            size_bytes: 1200,
        },
    );
    assert!(effects.iter().any(|e| matches!(
        e,
        FsmEffect::PersistDanmuSegment(rec) if rec.segment_index == 0
    )));

    let effects = fsm.step(
        clock.ctx(),
        FsmInput::SegmentCompleted {
            attempt_id,
            session_id: "sess-rev".into(),
            download_id: "dl-1".into(),
            segment_index: 0,
            segment_path: "/rec/seg0.flv".into(),
            started_at: Some(clock.wall),
            completed_at: clock.wall,
            duration_secs: 1.0,
            size_bytes: 10_000,
            effective_size_bytes: 10_000,
            split_reason_code: None,
            split_reason_details_json: None,
            split_by_format_change: false,
        },
    );
    assert!(effects.iter().any(|e| matches!(
        e,
        FsmEffect::RemoveDanmuSegmentOutput {
            session_id,
            danmu_path,
        } if session_id == "sess-rev" && danmu_path == "/rec/seg0.xml"
    )));
    assert!(effects.iter().any(|e| matches!(
        e,
        FsmEffect::DeleteDiscardedFiles {
            video_path: Some(v),
            danmu_path: Some(d),
        } if v == "/rec/seg0.flv" && d == "/rec/seg0.xml"
    )));

    fsm.step(clock.ctx(), FsmInput::Disable);
    assert!(matches!(fsm.monitor, MonitorPhase::Stopping(_)));
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::SegmentCompleted {
            attempt_id: attempt_id + 999,
            session_id: "sess-rev".into(),
            download_id: "dl-stale".into(),
            segment_index: 2,
            segment_path: "/rec/seg2.flv".into(),
            started_at: Some(clock.wall),
            completed_at: clock.wall,
            duration_secs: 60.0,
            size_bytes: 5_000_000,
            effective_size_bytes: 5_000_000,
            split_reason_code: None,
            split_reason_details_json: None,
            split_by_format_change: false,
        },
    );
    assert!(effects.iter().any(|e| matches!(
        e,
        FsmEffect::PersistVideoSegment(rec) if rec.segment_index == 2 && rec.size_bytes == 5_000_000
    )));
    assert!(effects.iter().any(|e| matches!(
        e,
        FsmEffect::NotifyPipeline(PipelineNotification::VideoSegmentCompleted {
            segment_index: 2,
            ..
        })
    )));
}

#[test]
fn danmu_control_title_stream_closed_and_reconnecting_timestamp_preservation() {
    let mut clock = TestClock::new();
    let mut cfg = base_config();
    cfg.record_danmu = true;
    let (mut fsm, attempt_id) = open_running_session(&clock, cfg, "sess-dm", None);
    fsm.step(
        clock.ctx(),
        FsmInput::DanmuCollectionStarted {
            session_id: "sess-dm".into(),
        },
    );

    let effects = fsm.step(
        clock.ctx(),
        FsmInput::DanmuControl {
            session_id: "sess-dm".into(),
            control: DanmuControlEvent::RoomInfoChanged {
                title: Some("New Room Title".into()),
                category: None,
                parent_category: None,
            },
        },
    );
    let (title_op, op) = find_db_commit(&effects).unwrap();
    assert!(matches!(
        op,
        DbCommitOp::AddSessionTitle { session_id, title, .. }
            if session_id == "sess-dm" && title == "New Room Title"
    ));
    fsm.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id: title_op,
            result: Ok(DbCommitOutcome::SessionTitleAdded { changed: true }),
        },
    );

    // Repeated DanmuReconnecting preserves the initial outage timestamp via `get_or_insert`.
    clock.advance(Duration::from_secs(5));
    let first_down = clock.now;
    fsm.step(
        clock.ctx(),
        FsmInput::DanmuReconnecting {
            session_id: "sess-dm".into(),
            attempt: 1,
        },
    );
    assert_eq!(fsm.runtime_view().danmu_link_down_since, Some(first_down));
    clock.advance(Duration::from_secs(10));
    fsm.step(
        clock.ctx(),
        FsmInput::DanmuReconnecting {
            session_id: "sess-dm".into(),
            attempt: 2,
        },
    );
    assert_eq!(fsm.runtime_view().danmu_link_down_since, Some(first_down));

    fsm.step(
        clock.ctx(),
        FsmInput::DanmuReconnected {
            session_id: "sess-dm".into(),
            attempts: 2,
            downtime_secs: 10,
        },
    );
    assert_eq!(fsm.runtime_view().danmu_link_down_since, None);
    assert!(matches!(
        &fsm.session,
        SessionSlot::Open(open) if matches!(open.danmu, DanmuSessionState::Running { link_down_since: None })
    ));

    // Stale session_id StreamClosed is ignored.
    assert!(
        fsm.step(
            clock.ctx(),
            FsmInput::DanmuControl {
                session_id: "old-session".into(),
                control: DanmuControlEvent::StreamClosed {
                    message: None,
                    action: None,
                },
            },
        )
        .is_empty()
    );

    // Active session_id StreamClosed stops attempt and commits DefinitiveOffline(DanmuStreamClosed).
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::DanmuControl {
            session_id: "sess-dm".into(),
            control: DanmuControlEvent::StreamClosed {
                message: Some("ended".into()),
                action: None,
            },
        },
    );
    assert!(effects.iter().any(|e| matches!(
        e,
        FsmEffect::StopAttempt {
            attempt_id: id,
            cause: DownloadStopCause::DanmuStreamClosed,
            ..
        } if *id == attempt_id
    )));
    let (_, end_op) = find_db_commit(&effects).unwrap();
    assert!(matches!(
        end_op,
        DbCommitOp::EndSession {
            cause: TerminalCause::DefinitiveOffline {
                signal: OfflineSignal::DanmuStreamClosed,
            },
            update_streamer_state: Some(StreamerState::NotLive),
            ..
        }
    ));
}
