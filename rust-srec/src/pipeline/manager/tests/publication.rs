use super::*;

use crate::database::models::JobPreset;
use crate::database::repositories::{
    SqliteJobPresetRepository, SqlxDagRepository, SqlxJobRepository,
};

async fn assert_context_publication_boundary(paired: bool, fail: bool) {
    let manager: PipelineManager = PipelineManager::with_repository(
        PipelineManagerConfig::default(),
        Arc::new(TestJobRepository::new()),
    );
    let segment_contexts = manager.dag_segment_contexts.clone();
    let paired_contexts = manager.paired_dag_contexts.clone();
    let observed = Arc::new(AtomicUsize::new(0));
    let observed_at_publish = observed.clone();
    let mut repository = TestDagRepository::new();
    repository.before_publish = Some(Box::new(move |dag| {
        // A worker may claim a root as soon as publish commits. Its completion must already
        // have the correct source context, even when the in-memory queue is not populated.
        if paired {
            let context = paired_contexts
                .get(&dag.id)
                .expect("paired registration precedes publication");
            assert_eq!(context.segment_index, 3);
            assert_eq!(context.session_id, "session");
        } else {
            let context = segment_contexts
                .get(&dag.id)
                .expect("segment registration precedes publication");
            assert_eq!(context.segment_index, 3);
            assert_eq!(context.session_id, "session");
        }
        observed_at_publish.fetch_add(1, Ordering::SeqCst);
        if fail {
            Err(Error::Database("injected publication failure".to_string()))
        } else {
            Ok(())
        }
    }));
    let repository = Arc::new(repository);
    let manager = manager.with_dag_repository(repository.clone());
    let definition = DagPipelineDefinition::new(
        "publication",
        vec![DagStep::new(
            "root",
            PipelineStep::inline("remux", serde_json::json!({})),
        )],
    );
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("input.flv");
    let _commands = if paired {
        manager
            .run_paired_segment_pipeline(
                PairedSegmentOutputs {
                    session_id: "session".to_string(),
                    streamer_id: "streamer".to_string(),
                    segment_index: 3,
                    video_outputs: vec![input],
                    danmu_outputs: vec![],
                },
                definition,
            )
            .await
    } else {
        manager
            .run_segment_pipeline(
                "session".to_string(),
                "streamer".to_string(),
                3,
                SourceType::Video,
                input,
                definition,
            )
            .await
    };
    assert_eq!(observed.load(Ordering::SeqCst), 1);
    let expected = usize::from(!fail);
    assert_eq!(repository.create_calls(), expected);
    assert_eq!(manager.job_queue.depth(), expected);
    assert_eq!(
        manager.dag_segment_contexts.len(),
        if paired { 0 } else { expected }
    );
    assert_eq!(
        manager.paired_dag_contexts.len(),
        if paired { expected } else { 0 }
    );
}

#[tokio::test]
async fn segment_context_rolls_back_only_on_failed_publication() {
    assert_context_publication_boundary(false, true).await;
    assert_context_publication_boundary(false, false).await;
}

#[tokio::test]
async fn paired_context_rolls_back_only_on_failed_publication() {
    assert_context_publication_boundary(true, true).await;
    assert_context_publication_boundary(true, false).await;
}

#[tokio::test]
async fn cancelling_pending_publication_does_not_assume_commit_failed() {
    let (pool, manager, _presets) = preset_manager().await;
    // Hold the only connection so publication suspends after registration, before it can
    // return a definitive result. A generic repository may also suspend during commit.
    let connection = pool.acquire().await.unwrap();
    let registered = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let state = registered.clone();
    let hook = Box::new(move |_: &str| {
        state.store(true, Ordering::SeqCst);
        Box::new(move || state.store(false, Ordering::SeqCst)) as PublicationRollback
    }) as BeforeRootJobsHook;
    let definition = DagPipelineDefinition::new(
        "pending publication",
        vec![DagStep::new(
            "root",
            PipelineStep::inline("remux", serde_json::json!({})),
        )],
    );
    let mut publication = Box::pin(manager.create_dag_pipeline_internal(
        "session",
        "streamer",
        vec!["input.flv".to_string()],
        definition,
        Some(hook),
        None,
    ));
    assert!(futures::poll!(publication.as_mut()).is_pending());
    assert!(registered.load(Ordering::SeqCst));
    drop(publication);
    assert!(
        registered.load(Ordering::SeqCst),
        "cancellation is not a confirmed publication error"
    );
    drop(connection);
    pool.close().await;
}

async fn preset_manager() -> (
    sqlx::SqlitePool,
    PipelineManager,
    Arc<SqliteJobPresetRepository>,
) {
    let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    crate::database::run_migrations(&pool).await.unwrap();
    let presets = Arc::new(SqliteJobPresetRepository::new(
        Arc::new(pool.clone()),
        Arc::new(pool.clone()),
    ));
    let manager = PipelineManager::with_repository(
        PipelineManagerConfig::default(),
        Arc::new(SqlxJobRepository::new(pool.clone(), pool.clone())),
    )
    .with_dag_repository(Arc::new(SqlxDagRepository::new(pool.clone(), pool.clone())))
    .with_preset_repository(presets.clone());
    (pool, manager, presets)
}

#[tokio::test]
async fn malformed_preset_prevents_root_publication_and_context_registration() {
    let (pool, manager, presets) = preset_manager().await;
    let mut preset = JobPreset::new(
        "broken-publication-preset",
        "remux",
        serde_json::Value::Null,
    );
    preset.config = r#"{"api_key":"private-preset-secret","#.to_string();
    presets.create_preset(&preset).await.unwrap();
    let hook_calls = Arc::new(AtomicUsize::new(0));
    let seen = hook_calls.clone();
    let before = Box::new(move |_: &str| {
        seen.fetch_add(1, Ordering::SeqCst);
        Box::new(|| {}) as PublicationRollback
    }) as BeforeRootJobsHook;
    let definition = DagPipelineDefinition::new(
        "invalid downstream preset",
        vec![
            DagStep::new("root", PipelineStep::inline("remux", serde_json::json!({}))),
            DagStep::with_dependencies(
                "downstream",
                PipelineStep::preset(&preset.name),
                vec!["root".to_string()],
            ),
        ],
    );
    let error = manager
        .create_dag_pipeline_internal(
            "session",
            "streamer",
            vec!["input.flv".to_string()],
            definition,
            Some(before),
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Validation(_)));
    let message = error.to_string();
    assert!(message.contains(&preset.name));
    assert!(!message.contains("private-preset-secret"));
    assert!(!message.contains("api_key"));
    assert_eq!(hook_calls.load(Ordering::SeqCst), 0);
    assert_eq!(manager.job_queue.depth(), 0);
    let counts: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM dag_execution), (SELECT COUNT(*) FROM dag_step_execution), (SELECT COUNT(*) FROM job)",
    ).fetch_one(&pool).await.unwrap();
    assert_eq!(counts, (0, 0, 0));
    pool.close().await;
}

#[tokio::test]
async fn preset_resolution_preserves_empty_valid_json_and_missing_preset_fallback() {
    let (pool, manager, presets) = preset_manager().await;
    for (index, text) in ["", "null", "{}", r#"{"timeout":7}"#, "[]"]
        .into_iter()
        .enumerate()
    {
        let mut preset = JobPreset::new(
            format!("publication-preset-{index}"),
            "remux",
            serde_json::Value::Null,
        );
        preset.config = text.to_string();
        presets.create_preset(&preset).await.unwrap();
        let resolved = manager
            .resolve_dag_step(&PipelineStep::preset(&preset.name))
            .await
            .unwrap();
        let PipelineStep::Inline { processor, config } = resolved else {
            panic!("preset must resolve inline")
        };
        assert_eq!(processor, "remux");
        assert_eq!(
            config,
            if text.is_empty() {
                serde_json::Value::Null
            } else {
                serde_json::from_str::<serde_json::Value>(text).unwrap()
            }
        );
    }
    let PipelineStep::Inline { processor, config } = manager
        .resolve_dag_step(&PipelineStep::preset("unregistered-preset-name"))
        .await
        .unwrap()
    else {
        panic!("missing preset must preserve processor-name fallback")
    };
    assert_eq!(processor, "unregistered-preset-name");
    assert!(config.is_null());
    pool.close().await;
}
