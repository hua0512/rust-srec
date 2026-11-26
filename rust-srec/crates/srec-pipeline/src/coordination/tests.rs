use super::*;

#[test]
fn live_and_recovered_artifacts_share_reservation_and_failed_fallback_behavior() {
    for recovered in [false, true] {
        for source in [SourceType::Video, SourceType::Danmu] {
            let mut session =
                SessionPipelineState::new("session".to_owned(), "streamer".to_owned());
            session.segment_pipeline = Some(DagPipelineDefinition::new(
                "segment",
                vec![srec_sqlite::models::DagStep::new(
                    "root",
                    srec_sqlite::models::PipelineStep::inline("remux", serde_json::json!({})),
                )],
            ));
            let deliver = |session: &mut SessionPipelineState, path: &str| {
                if recovered {
                    session.recover_source_artifact(source, 3, PathBuf::from(path))
                } else {
                    session.on_source_artifact(source, 3, PathBuf::from(path))
                }
            };
            let commands = deliver(&mut session, "first.flv");
            assert!(
                matches!(&commands[..], [PipelineCommand::CreateSegmentDag { segment_index: 3, input_path, .. }] if input_path == &PathBuf::from("first.flv"))
            );
            assert!(session.pending_segment_starts.contains(&(source, 3)));
            assert!(deliver(&mut session, "first.flv").is_empty());
            session.on_segment_dag_started(source, 3);
            session.on_segment_dag_failed(source, 3);
            assert!(deliver(&mut session, "second.flv").is_empty());
            let lane = session.segment_mut(3).artifact_mut(source);
            assert!(lane.dag_failed && lane.final_outputs_are_fallback);
            assert_eq!(
                lane.source_inputs,
                vec![PathBuf::from("first.flv"), PathBuf::from("second.flv")]
            );
            assert_eq!(lane.final_outputs, lane.source_inputs);
        }
    }
}
use srec_sqlite::models::job::{DagStep, PipelineStep};

fn empty_session_pipeline() -> DagPipelineDefinition {
    DagPipelineDefinition::new("session-complete", vec![])
}

fn non_empty_pipeline(name: &str) -> DagPipelineDefinition {
    DagPipelineDefinition::new(name, vec![DagStep::new("A", PipelineStep::preset("remux"))])
}

fn configure_event(
    session_id: &str,
    streamer_id: &str,
    danmu_enabled: bool,
    segment_pipeline: Option<DagPipelineDefinition>,
    paired_segment_pipeline: Option<DagPipelineDefinition>,
    session_complete_pipeline: Option<DagPipelineDefinition>,
) -> PipelineCoordinationEvent {
    PipelineCoordinationEvent::ConfigureSession {
        session_id: session_id.to_string(),
        streamer_id: streamer_id.to_string(),
        danmu_enabled,
        segment_pipeline,
        paired_segment_pipeline,
        session_complete_pipeline,
    }
}

fn end_event(session_id: &str, streamer_id: &str) -> PipelineCoordinationEvent {
    PipelineCoordinationEvent::SessionEnded {
        session_id: session_id.to_string(),
        streamer_id: streamer_id.to_string(),
        should_run_session_complete: true,
    }
}

fn persisted_event(session_id: &str) -> PipelineCoordinationEvent {
    PipelineCoordinationEvent::SessionEndPersisted {
        session_id: session_id.to_string(),
    }
}

#[test]
fn pipeline_coordinator_waits_when_danmu_finishes_before_video() {
    let coord = PipelineCoordinator::new();
    let session_id = "session1";
    let streamer_id = "streamer1";

    assert!(
        coord
            .apply_event(configure_event(
                session_id,
                streamer_id,
                true,
                None,
                None,
                Some(empty_session_pipeline()),
            ))
            .is_empty()
    );
    assert!(
        coord
            .apply_event(PipelineCoordinationEvent::DanmuCollectionStarted {
                session_id: session_id.to_string(),
                streamer_id: streamer_id.to_string(),
            })
            .is_empty()
    );
    assert!(
        coord
            .apply_event(PipelineCoordinationEvent::DanmuSegmentCompleted {
                session_id: session_id.to_string(),
                streamer_id: streamer_id.to_string(),
                segment_index: 0,
                path: PathBuf::from("/seg0.xml"),
            })
            .is_empty()
    );
    assert!(
        coord
            .apply_event(PipelineCoordinationEvent::DanmuCollectionStopped {
                session_id: session_id.to_string(),
            })
            .is_empty()
    );
    assert!(
        coord
            .apply_event(end_event(session_id, streamer_id))
            .is_empty()
    );
    assert!(coord.apply_event(persisted_event(session_id)).is_empty());

    let commands = coord.apply_event(PipelineCoordinationEvent::VideoSegmentCompleted {
        session_id: session_id.to_string(),
        streamer_id: streamer_id.to_string(),
        segment_index: 0,
        path: PathBuf::from("/seg0.flv"),
    });

    assert_eq!(commands.len(), 1);
    match &commands[0] {
        PipelineCommand::CreateSessionCompleteDag { outputs, .. } => {
            assert_eq!(outputs.video_outputs.len(), 1);
            assert_eq!(outputs.danmu_outputs.len(), 1);
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn pipeline_coordinator_waits_for_danmu_stop_after_video() {
    let coord = PipelineCoordinator::new();
    let session_id = "session1";
    let streamer_id = "streamer1";

    coord.apply_event(configure_event(
        session_id,
        streamer_id,
        true,
        None,
        None,
        Some(empty_session_pipeline()),
    ));
    coord.apply_event(PipelineCoordinationEvent::DanmuCollectionStarted {
        session_id: session_id.to_string(),
        streamer_id: streamer_id.to_string(),
    });
    coord.apply_event(PipelineCoordinationEvent::VideoSegmentCompleted {
        session_id: session_id.to_string(),
        streamer_id: streamer_id.to_string(),
        segment_index: 0,
        path: PathBuf::from("/seg0.flv"),
    });
    coord.apply_event(end_event(session_id, streamer_id));
    assert!(coord.apply_event(persisted_event(session_id)).is_empty());

    let commands = coord.apply_event(PipelineCoordinationEvent::DanmuCollectionStopped {
        session_id: session_id.to_string(),
    });

    assert!(matches!(
        commands.as_slice(),
        [PipelineCommand::CreateSessionCompleteDag { .. }]
    ));
}

#[test]
fn pipeline_coordinator_session_complete_is_exactly_once() {
    let coord = PipelineCoordinator::new();
    let session_id = "session1";
    let streamer_id = "streamer1";

    coord.apply_event(configure_event(
        session_id,
        streamer_id,
        false,
        None,
        None,
        Some(empty_session_pipeline()),
    ));
    coord.apply_event(PipelineCoordinationEvent::VideoSegmentCompleted {
        session_id: session_id.to_string(),
        streamer_id: streamer_id.to_string(),
        segment_index: 0,
        path: PathBuf::from("/seg0.flv"),
    });
    coord.apply_event(end_event(session_id, streamer_id));
    let commands = coord.apply_event(persisted_event(session_id));
    assert_eq!(commands.len(), 1);

    assert!(coord.apply_event(persisted_event(session_id)).is_empty());
    assert!(
        coord
            .apply_event(PipelineCoordinationEvent::VideoSegmentCompleted {
                session_id: session_id.to_string(),
                streamer_id: streamer_id.to_string(),
                segment_index: 0,
                path: PathBuf::from("/seg0.flv"),
            })
            .is_empty()
    );
}

#[test]
fn session_complete_acknowledges_only_after_dag_creation() {
    let coord = PipelineCoordinator::new();
    let session_id = "session1";
    let streamer_id = "streamer1";

    coord.apply_event(configure_event(
        session_id,
        streamer_id,
        false,
        None,
        None,
        Some(non_empty_pipeline("session-complete")),
    ));
    coord.apply_event(PipelineCoordinationEvent::VideoSegmentCompleted {
        session_id: session_id.to_string(),
        streamer_id: streamer_id.to_string(),
        segment_index: 0,
        path: PathBuf::from("/seg0.flv"),
    });
    coord.apply_event(end_event(session_id, streamer_id));
    assert!(matches!(
        coord.apply_event(persisted_event(session_id)).as_slice(),
        [PipelineCommand::CreateSessionCompleteDag { .. }]
    ));

    coord.apply_event(
        PipelineCoordinationEvent::SessionCompleteDagCreationFailed {
            session_id: session_id.to_string(),
        },
    );
    assert!(matches!(
        coord.apply_event(persisted_event(session_id)).as_slice(),
        [PipelineCommand::CreateSessionCompleteDag { .. }]
    ));

    coord.apply_event(PipelineCoordinationEvent::SessionCompleteDagStarted {
        session_id: session_id.to_string(),
        streamer_id: streamer_id.to_string(),
    });
    assert!(coord.apply_event(persisted_event(session_id)).is_empty());
}

#[test]
fn recovered_failed_segment_uses_artifact_without_recreating_dag() {
    let coord = PipelineCoordinator::new();
    let session_id = "session1";
    let streamer_id = "streamer1";
    coord.apply_event(configure_event(
        session_id,
        streamer_id,
        false,
        Some(non_empty_pipeline("segment")),
        None,
        Some(non_empty_pipeline("session-complete")),
    ));
    coord.apply_event(PipelineCoordinationEvent::RecoverSegmentDagFailed {
        session_id: session_id.to_string(),
        segment_index: 0,
        source: SourceType::Video,
    });

    assert!(
        coord
            .apply_event(PipelineCoordinationEvent::RecoverSourceArtifact {
                session_id: session_id.to_string(),
                streamer_id: streamer_id.to_string(),
                segment_index: 0,
                source: SourceType::Video,
                path: PathBuf::from("/seg0.flv"),
            })
            .is_empty()
    );
    coord.apply_event(end_event(session_id, streamer_id));
    assert!(matches!(
        coord
            .apply_event(persisted_event(session_id))
            .as_slice(),
        [PipelineCommand::CreateSessionCompleteDag { outputs, .. }]
            if outputs.video_outputs.len() == 1
                && outputs.video_outputs[0].path.as_path() == Path::new("/seg0.flv")
    ));
}

#[test]
fn pipeline_coordinator_failed_segment_dag_drains() {
    let coord = PipelineCoordinator::new();
    let session_id = "session1";
    let streamer_id = "streamer1";

    coord.apply_event(configure_event(
        session_id,
        streamer_id,
        false,
        Some(non_empty_pipeline("segment")),
        None,
        Some(empty_session_pipeline()),
    ));
    let commands = coord.apply_event(PipelineCoordinationEvent::VideoSegmentCompleted {
        session_id: session_id.to_string(),
        streamer_id: streamer_id.to_string(),
        segment_index: 0,
        path: PathBuf::from("/seg0.flv"),
    });
    assert!(matches!(
        commands.as_slice(),
        [PipelineCommand::CreateSegmentDag { .. }]
    ));
    coord.apply_event(PipelineCoordinationEvent::SegmentDagStarted {
        session_id: session_id.to_string(),
        streamer_id: streamer_id.to_string(),
        segment_index: 0,
        source: SourceType::Video,
    });
    coord.apply_event(PipelineCoordinationEvent::SessionEnded {
        session_id: session_id.to_string(),
        streamer_id: streamer_id.to_string(),
        should_run_session_complete: true,
    });
    coord.apply_event(persisted_event(session_id));

    let commands = coord.apply_event(PipelineCoordinationEvent::SegmentDagFailed {
        session_id: session_id.to_string(),
        segment_index: 0,
        source: SourceType::Video,
    });

    assert!(matches!(
        commands.as_slice(),
        [PipelineCommand::CreateSessionCompleteDag { .. }]
    ));
}

#[test]
fn pipeline_coordinator_failed_segment_dag_can_be_retried_before_finalization() {
    let coord = PipelineCoordinator::new();
    let session_id = "session1";
    let streamer_id = "streamer1";

    coord.apply_event(configure_event(
        session_id,
        streamer_id,
        false,
        Some(non_empty_pipeline("segment")),
        None,
        Some(empty_session_pipeline()),
    ));
    let commands = coord.apply_event(PipelineCoordinationEvent::VideoSegmentCompleted {
        session_id: session_id.to_string(),
        streamer_id: streamer_id.to_string(),
        segment_index: 0,
        path: PathBuf::from("/seg0.flv"),
    });
    assert!(matches!(
        commands.as_slice(),
        [PipelineCommand::CreateSegmentDag { .. }]
    ));
    coord.apply_event(PipelineCoordinationEvent::SegmentDagStarted {
        session_id: session_id.to_string(),
        streamer_id: streamer_id.to_string(),
        segment_index: 0,
        source: SourceType::Video,
    });

    assert!(
        coord
            .apply_event(PipelineCoordinationEvent::SegmentDagFailed {
                session_id: session_id.to_string(),
                segment_index: 0,
                source: SourceType::Video,
            })
            .is_empty()
    );

    coord.apply_event(PipelineCoordinationEvent::SegmentDagStarted {
        session_id: session_id.to_string(),
        streamer_id: streamer_id.to_string(),
        segment_index: 0,
        source: SourceType::Video,
    });
    coord.apply_event(PipelineCoordinationEvent::SessionEnded {
        session_id: session_id.to_string(),
        streamer_id: streamer_id.to_string(),
        should_run_session_complete: true,
    });
    assert!(
        coord.apply_event(persisted_event(session_id)).is_empty(),
        "retried segment DAG is in flight, so fallback output must not finalize the session"
    );

    let commands = coord.apply_event(PipelineCoordinationEvent::SegmentDagCompleted {
        session_id: session_id.to_string(),
        streamer_id: streamer_id.to_string(),
        segment_index: 0,
        source: SourceType::Video,
        outputs: vec![PathBuf::from("/seg0.mp4")],
    });

    match commands.as_slice() {
        [PipelineCommand::CreateSessionCompleteDag { outputs, .. }] => {
            assert_eq!(
                outputs.get_sorted_video_outputs(),
                [PathBuf::from("/seg0.mp4")]
            );
        }
        other => panic!("unexpected commands: {other:?}"),
    }
}

#[test]
fn pipeline_coordinator_paired_pipeline_waits_for_same_index() {
    let coord = PipelineCoordinator::new();
    let session_id = "session1";
    let streamer_id = "streamer1";

    coord.apply_event(configure_event(
        session_id,
        streamer_id,
        true,
        None,
        Some(non_empty_pipeline("paired")),
        None,
    ));
    assert!(
        coord
            .apply_event(PipelineCoordinationEvent::VideoSegmentCompleted {
                session_id: session_id.to_string(),
                streamer_id: streamer_id.to_string(),
                segment_index: 0,
                path: PathBuf::from("/seg0.flv"),
            })
            .is_empty()
    );
    assert!(
        coord
            .apply_event(PipelineCoordinationEvent::DanmuSegmentCompleted {
                session_id: session_id.to_string(),
                streamer_id: streamer_id.to_string(),
                segment_index: 1,
                path: PathBuf::from("/seg1.xml"),
            })
            .is_empty()
    );

    let commands = coord.apply_event(PipelineCoordinationEvent::DanmuSegmentCompleted {
        session_id: session_id.to_string(),
        streamer_id: streamer_id.to_string(),
        segment_index: 0,
        path: PathBuf::from("/seg0.xml"),
    });
    assert!(matches!(
        commands.as_slice(),
        [PipelineCommand::CreatePairedSegmentDag { .. }]
    ));
}

/// When the last segment's video DAG completes after the session has
/// already ended and persisted, `try_trigger_paired` emits a paired
/// command and `try_finalize` runs in the same reducer call. The
/// session-complete DAG must not fire alongside the paired DAG.
/// `pending_paired_starts` is the gate: it holds the segment index
/// from the moment `try_trigger_paired` emits until
/// `on_paired_dag_started` removes it.
#[test]
fn pipeline_coordinator_session_complete_waits_for_just_triggered_paired() {
    let coord = PipelineCoordinator::new();
    let session_id = "session1";
    let streamer_id = "streamer1";

    coord.apply_event(configure_event(
        session_id,
        streamer_id,
        true,
        Some(non_empty_pipeline("segment")),
        Some(non_empty_pipeline("paired")),
        Some(empty_session_pipeline()),
    ));
    coord.apply_event(PipelineCoordinationEvent::DanmuCollectionStarted {
        session_id: session_id.to_string(),
        streamer_id: streamer_id.to_string(),
    });

    // Source artifacts kick off per-segment DAGs.
    let commands = coord.apply_event(PipelineCoordinationEvent::VideoSegmentCompleted {
        session_id: session_id.to_string(),
        streamer_id: streamer_id.to_string(),
        segment_index: 0,
        path: PathBuf::from("/seg0.flv"),
    });
    assert!(matches!(
        commands.as_slice(),
        [PipelineCommand::CreateSegmentDag { .. }]
    ));
    let commands = coord.apply_event(PipelineCoordinationEvent::DanmuSegmentCompleted {
        session_id: session_id.to_string(),
        streamer_id: streamer_id.to_string(),
        segment_index: 0,
        path: PathBuf::from("/seg0.xml"),
    });
    assert!(matches!(
        commands.as_slice(),
        [PipelineCommand::CreateSegmentDag { .. }]
    ));

    // Both per-segment DAGs are in flight.
    coord.apply_event(PipelineCoordinationEvent::SegmentDagStarted {
        session_id: session_id.to_string(),
        streamer_id: streamer_id.to_string(),
        segment_index: 0,
        source: SourceType::Video,
    });
    coord.apply_event(PipelineCoordinationEvent::SegmentDagStarted {
        session_id: session_id.to_string(),
        streamer_id: streamer_id.to_string(),
        segment_index: 0,
        source: SourceType::Danmu,
    });

    // Session wraps up while the per-segment DAGs are still running.
    assert!(
        coord
            .apply_event(PipelineCoordinationEvent::DanmuCollectionStopped {
                session_id: session_id.to_string(),
            })
            .is_empty()
    );
    assert!(
        coord
            .apply_event(end_event(session_id, streamer_id))
            .is_empty()
    );
    assert!(coord.apply_event(persisted_event(session_id)).is_empty());

    // Danmu DAG drains first. Video is still in flight, so neither
    // a paired command nor session-complete can fire.
    let commands = coord.apply_event(PipelineCoordinationEvent::SegmentDagCompleted {
        session_id: session_id.to_string(),
        streamer_id: streamer_id.to_string(),
        segment_index: 0,
        source: SourceType::Danmu,
        outputs: vec![PathBuf::from("/seg0.xml")],
    });
    assert!(
        commands.is_empty(),
        "danmu drain alone must not trigger paired or session-complete: {commands:?}"
    );

    // Video DAG drains. `try_trigger_paired` emits the paired
    // command; `try_finalize` runs in the same call but must NOT
    // emit session-complete because `pending_paired_starts` now
    // holds segment 0.
    let commands = coord.apply_event(PipelineCoordinationEvent::SegmentDagCompleted {
        session_id: session_id.to_string(),
        streamer_id: streamer_id.to_string(),
        segment_index: 0,
        source: SourceType::Video,
        outputs: vec![PathBuf::from("/seg0.mp4")],
    });
    match commands.as_slice() {
        [PipelineCommand::CreatePairedSegmentDag { outputs, .. }] => {
            assert_eq!(outputs.segment_index, 0);
        }
        other => {
            panic!("expected only the paired command; session-complete must wait: {other:?}")
        }
    }

    // `PairedDagStarted` clears `pending_paired_starts`, but
    // `pending_paired_dags` becomes 1, so session-complete still
    // waits.
    coord.apply_event(PipelineCoordinationEvent::PairedDagStarted {
        session_id: session_id.to_string(),
        streamer_id: streamer_id.to_string(),
        segment_index: 0,
    });

    // Paired DAG finishes -> all gates clear -> session-complete fires.
    let commands = coord.apply_event(PipelineCoordinationEvent::PairedDagCompleted {
        session_id: session_id.to_string(),
    });
    assert!(matches!(
        commands.as_slice(),
        [PipelineCommand::CreateSessionCompleteDag { .. }]
    ));
}
