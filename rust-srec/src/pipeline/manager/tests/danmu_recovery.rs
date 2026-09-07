use super::*;

const SESSION: &str = "danmu-recovery";
const STREAMER: &str = "streamer-1";

type RecoveryManager = PipelineManager<SqlxConfigRepository, SqlxStreamerRepository>;

async fn manager(
    segment_pipeline: bool,
    paired_pipeline: bool,
) -> (
    RecoveryManager,
    Arc<TestSessionRepository>,
    Arc<TestDagRepository>,
) {
    let pipeline = DagPipelineDefinition::new(
        "recovery",
        vec![DagStep::new(
            "step",
            PipelineStep::inline("remux", serde_json::json!({})),
        )],
    );
    let (config_repo, streamer_repo, config_service) =
        configured_recovery_services(STREAMER, segment_pipeline.then_some(&pipeline), &pipeline)
            .await;
    let mut global = config_repo.get_global_config().await.unwrap();
    global.record_danmu = true;
    global.paired_segment_pipeline =
        paired_pipeline.then(|| serde_json::to_string(&pipeline).unwrap());
    config_repo.update_global_config(&global).await.unwrap();
    let sessions = Arc::new(TestSessionRepository::new(None));
    sessions.insert_session(test_session(SESSION, STREAMER, None));
    let dags = Arc::new(TestDagRepository::new());
    let manager = RecoveryManager::with_repository(
        PipelineManagerConfig::default(),
        Arc::new(TestJobRepository::new()),
    )
    .with_session_repository(sessions.clone())
    .with_streamer_repository(streamer_repo)
    .with_config_service(config_service)
    .with_dag_repository(dags.clone());
    (manager, sessions, dags)
}

fn add_video_and_xml(sessions: &TestSessionRepository, path: &Path, index: u32) {
    sessions.insert_segment(test_segment(SESSION, index, &path.to_string_lossy()));
    let mut output = MediaOutputDbModel::new(
        SESSION,
        path.with_extension("xml").to_string_lossy(),
        MediaFileType::DanmuXml,
        64,
    );
    output.id = format!("00000000-0000-0000-0000-{:012}", 999 + index);
    sessions.insert_output(output);
}

#[tokio::test]
async fn danmu_recovery_uses_stored_indices_and_creates_missing_dags_once() {
    let (manager, sessions, dags) = manager(true, false).await;
    let dir = tempfile::tempdir().unwrap();
    add_video_and_xml(&sessions, &dir.path().join("title-2026.flv"), 7);
    add_video_and_xml(&sessions, &dir.path().join("title-final.mp4"), 11);

    manager.recover_pipeline_coordination().await.unwrap();
    manager.recover_pipeline_coordination().await.unwrap();
    let recovered = dags.list_dags(None, Some(SESSION), 100, 0).await.unwrap();
    assert_eq!(recovered.len(), 4);
    for source in ["video", "danmu"] {
        let indices: HashSet<_> = recovered
            .iter()
            .filter(|dag| dag.segment_source.as_deref() == Some(source))
            .map(|dag| dag.segment_index)
            .collect();
        assert_eq!(indices, HashSet::from([Some(7), Some(11)]));
    }
    assert_eq!(dags.create_calls(), 4);
}

#[tokio::test]
async fn danmu_recovery_does_not_replay_completed_dags_for_arbitrary_titles() {
    let (manager, sessions, dags) = manager(true, false).await;
    let dir = tempfile::tempdir().unwrap();
    for (index, name) in [(7, "title-2026.flv"), (11, "title-final.mp4")] {
        add_video_and_xml(&sessions, &dir.path().join(name), index);
        dags.insert(test_coordination_dag(
            SESSION,
            STREAMER,
            DagExecutionStatus::Completed,
            "danmu",
            index,
        ));
    }

    manager.recover_pipeline_coordination().await.unwrap();
    manager.recover_pipeline_coordination().await.unwrap();
    let recovered = dags.list_dags(None, Some(SESSION), 100, 0).await.unwrap();
    assert_eq!(
        dags.create_calls(),
        2,
        "only the missing video DAGs are created"
    );
    let danmu: Vec<_> = recovered
        .iter()
        .filter(|dag| dag.segment_source.as_deref() == Some("danmu"))
        .collect();
    assert_eq!(danmu.len(), 2);
    assert!(
        danmu
            .iter()
            .all(|dag| dag.get_status() == Some(DagExecutionStatus::Completed))
    );
}

#[tokio::test]
async fn danmu_recovery_skips_unmatched_and_ambiguous_historical_paths() {
    let (manager, sessions, dags) = manager(true, false).await;
    let dir = tempfile::tempdir().unwrap();
    for (index, extension) in [(4, "flv"), (5, "mp4")] {
        let path = dir.path().join(format!("same-title-90.{extension}"));
        sessions.insert_segment(test_segment(SESSION, index, &path.to_string_lossy()));
    }
    for path in [
        dir.path().join("same-title-90.xml"),
        dir.path().join("unmatched-title-91.xml"),
        dir.path().join("other-directory/same-title-90.xml"),
    ] {
        let output =
            MediaOutputDbModel::new(SESSION, path.to_string_lossy(), MediaFileType::DanmuXml, 64);
        sessions.insert_output(output);
    }
    manager.recover_pipeline_coordination().await.unwrap();
    manager.recover_pipeline_coordination().await.unwrap();
    let recovered = dags.list_dags(None, Some(SESSION), 100, 0).await.unwrap();
    assert_eq!(recovered.len(), 2);
    assert!(
        recovered
            .iter()
            .all(|dag| dag.segment_source.as_deref() == Some("video"))
    );
    assert_eq!(
        sessions
            .get_media_outputs_for_session(SESSION)
            .await
            .unwrap()
            .len(),
        3
    );
}

#[tokio::test]
async fn danmu_recovery_pairs_original_paths_at_the_stored_segment_index() {
    let (manager, sessions, dags) = manager(false, true).await;
    let dir = tempfile::tempdir().unwrap();
    let video = dir.path().join("title-2026.flv");
    add_video_and_xml(&sessions, &video, 7);
    manager.recover_pipeline_coordination().await.unwrap();
    manager.recover_pipeline_coordination().await.unwrap();
    let recovered = dags.list_dags(None, Some(SESSION), 100, 0).await.unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].segment_source.as_deref(), Some("paired"));
    assert_eq!(recovered[0].segment_index, Some(7));
    let manifest = tokio::fs::read(dir.path().join(format!("segment_{SESSION}_7_inputs.json")))
        .await
        .unwrap();
    let manifest: serde_json::Value = serde_json::from_slice(&manifest).unwrap();
    assert_eq!(manifest["segment_index"], 7);
    assert_eq!(
        manifest["video_inputs"],
        serde_json::json!([video.to_string_lossy()])
    );
    assert_eq!(
        manifest["danmu_inputs"],
        serde_json::json!([video.with_extension("xml").to_string_lossy()])
    );
}

#[tokio::test]
async fn danmu_recovery_stops_ended_collection_even_when_paths_cannot_be_matched() {
    for ambiguous in [false, true] {
        let (manager, sessions, dags) = manager(false, false).await;
        let dir = tempfile::tempdir().unwrap();
        sessions.insert_session(test_session(
            SESSION,
            STREAMER,
            Some(chrono::Utc::now().timestamp_millis()),
        ));
        let video = dir.path().join("same-title-90.flv");
        sessions.insert_segment(test_segment(SESSION, 4, &video.to_string_lossy()));
        let xml = if ambiguous {
            sessions.insert_segment(test_segment(
                SESSION,
                5,
                &video.with_extension("mp4").to_string_lossy(),
            ));
            video.with_extension("xml")
        } else {
            dir.path().join("unmatched-91.xml")
        };
        sessions.insert_output(MediaOutputDbModel::new(
            SESSION,
            xml.to_string_lossy(),
            MediaFileType::DanmuXml,
            64,
        ));
        // Recovery can run after collection activity has already reached the reducer.
        // Unmatched artifacts must still release that known collection-completion gate.
        manager.pipeline_coordinator.apply_event_inline(
            PipelineCoordinationEvent::DanmuCollectionStarted {
                session_id: SESSION.to_owned(),
                streamer_id: STREAMER.to_owned(),
            },
        );
        manager.recover_pipeline_coordination().await.unwrap();
        manager.recover_pipeline_coordination().await.unwrap();
        let recovered = dags.list_dags(None, Some(SESSION), 100, 0).await.unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(
            recovered[0].segment_source.as_deref(),
            Some("session_complete")
        );
        assert!(
            sessions
                .session_complete_dispatched
                .lock()
                .unwrap()
                .contains(SESSION)
        );
    }
}
