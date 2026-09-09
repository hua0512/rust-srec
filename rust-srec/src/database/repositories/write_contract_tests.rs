//! Full-row and transaction contracts exercised through the repository entrypoints.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::SqlitePool;

use super::*;
use crate::database::models::{
    DagExecutionDbModel, DagStepExecutionDbModel, JobDbModel, JobStatus, LiveSessionDbModel,
    MediaOutputDbModel, SessionSegmentDbModel, StreamerDbModel,
};

async fn fixture() -> SqlitePool {
    let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    crate::database::run_migrations(&pool).await.unwrap();
    let mut streamer =
        StreamerDbModel::new("Writer", "https://www.twitch.tv/writer", "platform-twitch");
    streamer.id = "streamer".into();
    SqlxStreamerRepository::new(pool.clone(), pool.clone())
        .create_streamer(&streamer)
        .await
        .unwrap();
    let mut session = LiveSessionDbModel::new("streamer").with_streamer_name("Writer snapshot");
    session.id = "session".into();
    sessions(&pool).create_session(&session).await.unwrap();
    pool
}

fn jobs(pool: &SqlitePool) -> SqlxJobRepository {
    SqlxJobRepository::new(pool.clone(), pool.clone())
}
fn dags(pool: &SqlitePool) -> SqlxDagRepository {
    SqlxDagRepository::new(pool.clone(), pool.clone())
}
fn sessions(pool: &SqlitePool) -> SqlxSessionRepository {
    SqlxSessionRepository::new(pool.clone(), pool.clone())
}

fn assert_row<T: Serialize>(actual: &T, expected: &T) {
    assert_eq!(
        serde_json::to_value(actual).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
}

fn dag(id: &str) -> DagExecutionDbModel {
    DagExecutionDbModel {
        id: id.into(),
        dag_definition: r#"{"steps":[],"extension":"preserve"}"#.into(),
        status: "PROCESSING".into(),
        streamer_id: Some("streamer".into()),
        session_id: Some("session".into()),
        segment_index: Some(17),
        segment_source: Some("video".into()),
        created_at: 1_710_000_000_123,
        updated_at: 1_710_000_000_456,
        completed_at: Some(1_710_000_000_789),
        error: Some("prior error".into()),
        total_steps: 3,
        completed_steps: 2,
        failed_steps: 1,
    }
}

fn step(id: &str, dag_id: &str) -> DagStepExecutionDbModel {
    DagStepExecutionDbModel {
        id: id.into(),
        dag_id: dag_id.into(),
        step_id: format!("logical-{id}"),
        job_id: None,
        status: "PENDING".into(),
        depends_on_step_ids: r#"["previous-step"]"#.into(),
        outputs: Some(r#"["persisted/output.flv"]"#.into()),
        created_at: 1_710_000_001_123,
        updated_at: 1_710_000_001_456,
    }
}

fn job(id: &str, step_id: Option<&str>, seed: i64) -> JobDbModel {
    JobDbModel {
        id: id.into(),
        job_type: format!("processor-{seed}"),
        status: "PROCESSING".into(),
        config: format!(r#"{{"config":{seed}}}"#),
        state: format!(r#"{{"state":{seed}}}"#),
        created_at: 1_710_000_002_123 + seed,
        updated_at: 1_710_000_002_456 + seed,
        input: Some(format!("input-{seed}.flv")),
        outputs: Some(format!(r#"["output-{seed}.mp4"]"#)),
        priority: 7,
        streamer_id: Some("streamer".into()),
        session_id: Some("session".into()),
        started_at: Some(1_710_000_003_123 + seed),
        completed_at: Some(1_710_000_003_456 + seed),
        error: Some(format!("error-{seed}")),
        retry_count: 3,
        pipeline_id: Some(format!("pipeline-{seed}")),
        execution_info: Some(format!(r#"{{"execution":{seed}}}"#)),
        duration_secs: Some(seed as f64 + 0.25),
        queue_wait_secs: Some(seed as f64 + 0.5),
        dag_step_execution_id: step_id.map(str::to_owned),
    }
}

fn media(id: &str, path: &str, bytes: i64) -> MediaOutputDbModel {
    MediaOutputDbModel {
        id: id.into(),
        session_id: "session".into(),
        parent_media_output_id: None,
        file_path: path.into(),
        file_type: "VIDEO".into(),
        size_bytes: bytes,
        created_at: 1_710_000_004_123,
    }
}

fn segment(id: &str, path: &str, index: i64) -> SessionSegmentDbModel {
    SessionSegmentDbModel {
        id: id.into(),
        session_id: "session".into(),
        segment_index: index,
        file_path: path.into(),
        duration_secs: 3.25,
        size_bytes: 99,
        split_reason_code: Some("duration".into()),
        split_reason_details_json: Some(r#"{"limit":3,"extension":true}"#.into()),
        created_at: Some(1_710_000_005_123),
        completed_at: Some(1_710_000_005_456),
        persisted_at: 1_710_000_005_789,
    }
}

async fn read_segment(pool: &SqlitePool, id: &str) -> SessionSegmentDbModel {
    sqlx::query_as("SELECT * FROM session_segments WHERE id = ?")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn complete_job_and_dag_rows_survive_all_three_job_creation_paths() {
    let pool = fixture().await;
    let dag_repo = dags(&pool);
    let job_repo = jobs(&pool);
    let standalone_dag = dag("standalone");
    dag_repo.create_dag(&standalone_dag).await.unwrap();
    assert_row(
        &dag_repo.get_dag(&standalone_dag.id).await.unwrap(),
        &standalone_dag,
    );
    let standalone_step = step("standalone-step", &standalone_dag.id);
    dag_repo.create_step(&standalone_step).await.unwrap();
    let standalone = job("standalone-job", Some(&standalone_step.id), 1);
    job_repo.create_job(&standalone).await.unwrap();
    assert_row(
        &job_repo.get_job(&standalone.id).await.unwrap(),
        &standalone,
    );

    let published_dag = dag("published");
    let mut root_step = step("root-step", &published_dag.id);
    let root = job("root-job", Some(&root_step.id), 2);
    root_step.job_id = Some(root.id.clone());
    root_step.status = "PROCESSING".into();
    dag_repo
        .publish_dag(
            &published_dag,
            std::slice::from_ref(&root_step),
            std::slice::from_ref(&root),
        )
        .await
        .unwrap();
    assert_row(
        &dag_repo.get_dag(&published_dag.id).await.unwrap(),
        &published_dag,
    );
    assert_row(&job_repo.get_job(&root.id).await.unwrap(), &root);
    root_step.updated_at = published_dag.updated_at;
    assert_row(&dag_repo.get_step(&root_step.id).await.unwrap(), &root_step);

    for status in ["PENDING", "BLOCKED"] {
        let mut blocked = step(&format!("later-step-{status}"), &published_dag.id);
        blocked.status = status.into();
        dag_repo.create_step(&blocked).await.unwrap();
        let later = job(&format!("later-job-{status}"), Some(&blocked.id), 3);
        let before = crate::database::time::now_ms();
        dag_repo
            .create_job_for_step(&blocked.id, &later)
            .await
            .unwrap();
        let after = crate::database::time::now_ms();
        assert_row(&job_repo.get_job(&later.id).await.unwrap(), &later);
        let attached = dag_repo.get_step(&blocked.id).await.unwrap();
        assert!((before..=after).contains(&attached.updated_at));
        blocked.updated_at = attached.updated_at;
        blocked.job_id = Some(later.id);
        blocked.status = "PROCESSING".into();
        assert_row(&attached, &blocked);
    }
}

#[tokio::test]
async fn step_insert_modes_preserve_fields_and_publication_clears_attachment() {
    let pool = fixture().await;
    let repo = dags(&pool);
    let parent = dag("steps");
    repo.create_dag(&parent).await.unwrap();
    let existing = job("existing-job", None, 4);
    jobs(&pool).create_job(&existing).await.unwrap();
    for batch in [false, true] {
        let mut row = step(if batch { "batch" } else { "single" }, &parent.id);
        row.job_id = Some(existing.id.clone());
        row.status = "FAILED".into();
        if batch {
            repo.create_steps(std::slice::from_ref(&row)).await.unwrap();
        } else {
            repo.create_step(&row).await.unwrap();
        }
        assert_row(&repo.get_step(&row.id).await.unwrap(), &row);
    }
    repo.create_steps(&[]).await.unwrap();
    let published = dag("normalized");
    let mut attached = step("normalized-step", &published.id);
    attached.job_id = Some(existing.id);
    attached.status = "FAILED".into();
    let mut unattached = step("unattached-step", &published.id);
    unattached.status = "BLOCKED".into();
    unattached.outputs = None;
    repo.publish_dag(&published, &[attached.clone(), unattached.clone()], &[])
        .await
        .unwrap();
    attached.job_id = None;
    attached.status = "PENDING".into();
    assert_row(&repo.get_step(&attached.id).await.unwrap(), &attached);
    assert_row(&repo.get_step(&unattached.id).await.unwrap(), &unattached);

    let first = step("rolled-step", &parent.id);
    assert!(
        repo.create_steps(&[first.clone(), first.clone()])
            .await
            .is_err()
    );
    assert!(matches!(
        repo.get_step(&first.id).await,
        Err(crate::Error::NotFound { .. })
    ));
}

#[tokio::test]
async fn complete_job_updates_preserve_creation_time_and_guard_every_assignment() {
    let pool = fixture().await;
    let repo = jobs(&pool);
    let parent = dag("update-dag");
    dags(&pool).create_dag(&parent).await.unwrap();
    let link = step("update-step", &parent.id);
    dags(&pool).create_step(&link).await.unwrap();
    let original = job("update-job", Some(&link.id), 10);
    repo.create_job(&original).await.unwrap();
    let mut changed = job(&original.id, Some(&link.id), 20);
    changed.status = "COMPLETED".into();
    changed.priority = 11;
    changed.retry_count = 7;
    assert_eq!(
        repo.update_job_if_status(&changed, JobStatus::Pending)
            .await
            .unwrap(),
        0
    );
    assert_row(&repo.get_job(&original.id).await.unwrap(), &original);
    let before = crate::database::time::now_ms();
    assert_eq!(
        repo.update_job_if_status(&changed, JobStatus::Processing)
            .await
            .unwrap(),
        1
    );
    let updated = repo.get_job(&original.id).await.unwrap();
    assert!((before..=crate::database::time::now_ms()).contains(&updated.updated_at));
    changed.created_at = original.created_at;
    changed.updated_at = updated.updated_at;
    assert_row(&updated, &changed);

    changed.input = None;
    changed.outputs = None;
    changed.streamer_id = None;
    changed.session_id = None;
    changed.started_at = None;
    changed.completed_at = None;
    changed.error = None;
    changed.pipeline_id = None;
    changed.execution_info = None;
    changed.duration_secs = None;
    changed.queue_wait_secs = None;
    changed.dag_step_execution_id = None;
    changed.created_at = 999;
    changed.updated_at = 123;
    let before = crate::database::time::now_ms();
    repo.update_job(&changed).await.unwrap();
    let updated = repo.get_job(&original.id).await.unwrap();
    assert!((before..=crate::database::time::now_ms()).contains(&updated.updated_at));
    changed.created_at = original.created_at;
    changed.updated_at = updated.updated_at;
    assert_row(&updated, &changed);
    changed.id = "missing-job".into();
    assert_eq!(
        repo.update_job_if_status(&changed, JobStatus::Completed)
            .await
            .unwrap(),
        0
    );
    repo.update_job(&changed).await.unwrap();
}

#[tokio::test]
async fn failed_root_and_later_attachment_roll_back_inserted_jobs() {
    let pool = fixture().await;
    let repo = dags(&pool);
    let parent = dag("failed-publication");
    let mut blocked = step("blocked-root", &parent.id);
    blocked.status = "BLOCKED".into();
    let root = job("rolled-root-job", Some(&blocked.id), 1);
    assert!(
        repo.publish_dag(
            &parent,
            std::slice::from_ref(&blocked),
            std::slice::from_ref(&root)
        )
        .await
        .is_err()
    );
    assert!(matches!(
        repo.get_dag(&parent.id).await,
        Err(crate::Error::NotFound { .. })
    ));
    assert!(matches!(
        repo.get_step(&blocked.id).await,
        Err(crate::Error::NotFound { .. })
    ));
    assert!(matches!(
        jobs(&pool).get_job(&root.id).await,
        Err(crate::Error::NotFound { .. })
    ));

    let external_dag = dag("external");
    repo.create_dag(&external_dag).await.unwrap();
    let external_step = step("external-step", &external_dag.id);
    repo.create_step(&external_step).await.unwrap();
    for absent_step_id in [false, true] {
        let invalid = dag(if absent_step_id {
            "missing-root-id"
        } else {
            "wrong-root-dag"
        });
        let candidate = job(
            &format!("job-{}", invalid.id),
            if absent_step_id {
                None
            } else {
                Some(&external_step.id)
            },
            5,
        );
        assert!(
            repo.publish_dag(&invalid, &[], std::slice::from_ref(&candidate))
                .await
                .is_err()
        );
        assert!(matches!(
            repo.get_dag(&invalid.id).await,
            Err(crate::Error::NotFound { .. })
        ));
        assert!(matches!(
            jobs(&pool).get_job(&candidate.id).await,
            Err(crate::Error::NotFound { .. })
        ));
        assert_row(
            &repo.get_step(&external_step.id).await.unwrap(),
            &external_step,
        );
    }

    for (index, parent_status, step_status, occupied) in [
        (0, "COMPLETED", "PENDING", false),
        (1, "FAILED", "BLOCKED", false),
        (2, "CANCELLED", "PENDING", false),
        (3, "PROCESSING", "COMPLETED", false),
        (4, "PROCESSING", "PENDING", true),
    ] {
        let mut parent = dag(&format!("guard-{index}"));
        parent.status = parent_status.into();
        repo.create_dag(&parent).await.unwrap();
        let mut target = step(&format!("guard-step-{index}"), &parent.id);
        target.status = step_status.into();
        if occupied {
            let old = job("occupying-job", None, 2);
            jobs(&pool).create_job(&old).await.unwrap();
            target.job_id = Some(old.id);
        }
        repo.create_step(&target).await.unwrap();
        let candidate = job(&format!("rejected-{index}"), Some(&target.id), 3);
        assert!(
            repo.create_job_for_step(&target.id, &candidate)
                .await
                .is_err()
        );
        assert!(matches!(
            jobs(&pool).get_job(&candidate.id).await,
            Err(crate::Error::NotFound { .. })
        ));
        assert_row(&repo.get_step(&target.id).await.unwrap(), &target);
    }
}

#[tokio::test]
async fn session_creation_and_end_modes_preserve_raw_values_defaults_and_idempotence() {
    let pool = fixture().await;
    let repo = sessions(&pool);
    let raw = LiveSessionDbModel {
        id: "raw".into(),
        streamer_id: None,
        streamer_name: None,
        start_time: -1234,
        end_time: Some(9876),
        titles: Some(r#"[{"title":"raw","ts":42,"extra":true}]"#.into()),
        total_size_bytes: 77,
    };
    repo.create_session(&raw).await.unwrap();
    assert_row(&repo.get_session(&raw.id).await.unwrap(), &raw);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT session_complete_dispatched FROM live_sessions WHERE id = 'raw'"
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        0
    );
    repo.end_session("session", 1000).await.unwrap();
    let started = DateTime::<Utc>::from_timestamp_millis(1_710_000_007_123).unwrap();
    let mut tx = crate::database::begin_immediate(&pool).await.unwrap();
    SessionTxOps::create_session(
        &mut tx,
        "live",
        "streamer",
        "Snapshot 名称",
        started,
        "Title 名称",
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let live = LiveSessionDbModel {
        id: "live".into(),
        streamer_id: Some("streamer".into()),
        streamer_name: Some("Snapshot 名称".into()),
        start_time: started.timestamp_millis(),
        end_time: None,
        titles: Some(
            serde_json::to_string(&[crate::database::models::TitleEntry {
                ts: started.timestamp_millis(),
                title: "Title 名称".into(),
            }])
            .unwrap(),
        ),
        total_size_bytes: 0,
    };
    assert_row(&repo.get_session("live").await.unwrap(), &live);
    let ended = started + chrono::Duration::milliseconds(321);
    let mut tx = crate::database::begin_immediate(&pool).await.unwrap();
    assert_eq!(
        SessionTxOps::end_session(&mut tx, "live", ended)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        SessionTxOps::end_session(&mut tx, "raw", ended)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        SessionTxOps::end_session(&mut tx, "missing", ended)
            .await
            .unwrap(),
        0
    );
    tx.commit().await.unwrap();
    assert_row(&repo.get_session("raw").await.unwrap(), &raw);
    sqlx::query("UPDATE live_sessions SET total_size_bytes = 999, session_complete_dispatched = 1 WHERE id = 'live'").execute(&pool).await.unwrap();
    let mut tx = crate::database::begin_immediate(&pool).await.unwrap();
    assert_eq!(
        SessionTxOps::end_session(&mut tx, "live", ended + chrono::Duration::seconds(1))
            .await
            .unwrap(),
        0
    );
    tx.commit().await.unwrap();
    assert_eq!(
        repo.get_session("live").await.unwrap().total_size_bytes,
        999
    );
    repo.end_session("live", i64::MAX).await.unwrap();
    let result = repo.get_session("live").await.unwrap();
    assert_eq!(result.end_time, Some(i64::MAX));
    assert_eq!(result.total_size_bytes, 0);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT session_complete_dispatched FROM live_sessions WHERE id = 'live'"
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        1
    );
    repo.end_session("missing", i64::MAX).await.unwrap();
}

#[tokio::test]
async fn media_and_segment_rows_preserve_all_fields_and_account_size_once() {
    let pool = fixture().await;
    let repo = sessions(&pool);
    let parent = media("parent", "parent.flv", 3);
    repo.create_media_output(&parent).await.unwrap();
    for combined in [false, true] {
        let mut output = media(
            if combined { "combined" } else { "single" },
            if combined {
                "combined.flv"
            } else {
                "single.flv"
            },
            101,
        );
        output.parent_media_output_id = Some(parent.id.clone());
        output.file_type = "THUMBNAIL".into();
        let mut row = segment(&output.id, &output.file_path, if combined { 2 } else { 1 });
        if !combined {
            row.created_at = None;
            row.completed_at = None;
        }
        if combined {
            repo.create_segment_output(&output, &row).await.unwrap();
        } else {
            repo.create_media_output(&output).await.unwrap();
            let before = repo.get_session("session").await.unwrap().total_size_bytes;
            repo.create_session_segment(&row).await.unwrap();
            assert_eq!(
                repo.get_session("session").await.unwrap().total_size_bytes,
                before
            );
        }
        assert_row(&repo.get_media_output(&output.id).await.unwrap(), &output);
        assert_row(&read_segment(&pool, &row.id).await, &row);
    }
    assert_eq!(
        repo.get_session("session").await.unwrap().total_size_bytes,
        205
    );
    let output = media("bad-pair", "bad.flv", 11);
    let mut mismatched = segment("bad-pair", "other.flv", 3);
    assert!(
        repo.create_segment_output(&output, &mismatched)
            .await
            .is_err()
    );
    mismatched.file_path = output.file_path.clone();
    mismatched.session_id = "other-session".into();
    assert!(
        repo.create_segment_output(&output, &mismatched)
            .await
            .is_err()
    );
    assert!(matches!(
        repo.get_media_output(&output.id).await,
        Err(crate::Error::NotFound { .. })
    ));
    assert_eq!(
        repo.get_session("session").await.unwrap().total_size_bytes,
        205
    );
}

#[tokio::test]
async fn media_size_and_segment_failures_roll_back_every_related_write() {
    let pool = fixture().await;
    let repo = sessions(&pool);
    sqlx::query("CREATE TRIGGER reject_total BEFORE UPDATE OF total_size_bytes ON live_sessions BEGIN SELECT RAISE(ABORT, 'reject total'); END").execute(&pool).await.unwrap();
    let output = media("rejected-media", "reject.flv", 41);
    assert!(repo.create_media_output(&output).await.is_err());
    assert!(matches!(
        repo.get_media_output(&output.id).await,
        Err(crate::Error::NotFound { .. })
    ));
    sqlx::query("DROP TRIGGER reject_total")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("CREATE TRIGGER reject_segment BEFORE INSERT ON session_segments BEGIN SELECT RAISE(ABORT, 'reject segment'); END").execute(&pool).await.unwrap();
    assert!(
        repo.create_segment_output(&output, &segment("rejected-segment", &output.file_path, 1))
            .await
            .is_err()
    );
    assert!(matches!(
        repo.get_media_output(&output.id).await,
        Err(crate::Error::NotFound { .. })
    ));
    assert_eq!(
        repo.get_session("session").await.unwrap().total_size_bytes,
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM session_segments")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn connection_helpers_do_not_commit_the_callers_transaction() {
    let pool = fixture().await;
    let persisted = job("persisted-job", None, 10);
    jobs(&pool).create_job(&persisted).await.unwrap();
    let mut tx = crate::database::begin_immediate(&pool).await.unwrap();
    assert_eq!(
        job::writes::update_job(
            &mut tx,
            &job(&persisted.id, None, 20),
            333,
            job::writes::UpdateCondition::Any
        )
        .await
        .unwrap(),
        1
    );
    assert_eq!(
        session_tx::writes::end_session(
            &mut tx,
            "session",
            i64::MAX,
            session_tx::writes::EndCondition::Any
        )
        .await
        .unwrap(),
        1
    );
    job::writes::insert_job(&mut tx, &job("rolled-job", None, 1))
        .await
        .unwrap();
    let row = LiveSessionDbModel {
        id: "rolled-session".into(),
        streamer_id: None,
        streamer_name: Some("history".into()),
        start_time: 123,
        end_time: None,
        titles: None,
        total_size_bytes: 0,
    };
    session_tx::writes::insert_session(&mut tx, &row)
        .await
        .unwrap();
    let mut output = media("rolled-media", "rolled.flv", 51);
    output.session_id = row.id.clone();
    session_tx::writes::insert_media_output(&mut tx, &output)
        .await
        .unwrap();
    let mut part = segment("rolled-segment", &output.file_path, 1);
    part.session_id = row.id.clone();
    session_tx::writes::insert_segment(&mut tx, &part)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    assert_row(
        &jobs(&pool).get_job(&persisted.id).await.unwrap(),
        &persisted,
    );
    assert!(
        sessions(&pool)
            .get_session("session")
            .await
            .unwrap()
            .end_time
            .is_none()
    );
    assert!(matches!(
        jobs(&pool).get_job("rolled-job").await,
        Err(crate::Error::NotFound { .. })
    ));
    assert!(matches!(
        sessions(&pool).get_session(&row.id).await,
        Err(crate::Error::NotFound { .. })
    ));
    assert!(matches!(
        sessions(&pool).get_media_output(&output.id).await,
        Err(crate::Error::NotFound { .. })
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM session_segments")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
}
