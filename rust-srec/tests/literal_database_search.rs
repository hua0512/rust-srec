use std::collections::BTreeSet;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use rust_srec::database::models::{
    JobDbModel, JobFilters, JobPreset, JobStatus, LiveSessionDbModel, MediaFileType,
    MediaOutputDbModel, NotificationEventLogDbModel, OutputFilters, Pagination, PipelinePreset,
    SessionFilters, StreamerDbModel,
};
use rust_srec::database::repositories::{
    JobPresetFilters, JobPresetRepository, JobRepository, NotificationRepository,
    PipelinePresetFilters, PipelinePresetRepository, SessionRepository, SqliteJobPresetRepository,
    SqlitePipelinePresetRepository, SqlxJobRepository, SqlxNotificationRepository,
    SqlxSessionRepository, SqlxStreamerRepository, StreamerRepository,
};
use rust_srec::database::{init_pool_with_size, run_migrations};
use sqlx::SqlitePool;

const VALUES: [&str; 10] = [
    "audio_extract",
    "audioXextract",
    "ratio%done",
    "ratioXdone",
    r"folder\clip",
    "folderclip",
    "MiXeD中Ä",
    "mixed中ä",
    r"escape\%_tail",
    r"escape\XYZtail",
];

const CASES: &[(&str, &[usize])] = &[
    ("_", &[0, 8]),
    ("%", &[2, 8]),
    (r"\", &[4, 8, 9]),
    (r"\%_", &[8]),
    ("AUDIO_EXTRACT", &[0]),
    ("mixed中Ä", &[6]),
    ("mixed中ä", &[7]),
    ("中", &[6, 7]),
    ("", &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9]),
    ("absent", &[]),
];

async fn pool() -> SqlitePool {
    let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
    run_migrations(&pool).await.unwrap();
    pool
}

fn assert_ids(actual: impl IntoIterator<Item = String>, ids: &[String], expected: &[usize]) {
    assert_eq!(
        actual.into_iter().collect::<BTreeSet<_>>(),
        expected.iter().map(|&index| ids[index].clone()).collect()
    );
}

async fn streamer(pool: &SqlitePool, index: usize, name: &str) -> String {
    let mut model = StreamerDbModel::new(
        name,
        format!("https://example.com/{index}"),
        "platform-twitch",
    );
    model.id = format!("streamer-{index}");
    SqlxStreamerRepository::new(pool.clone(), pool.clone())
        .create_streamer(&model)
        .await
        .unwrap();
    model.id
}

#[tokio::test]
async fn job_search_literals_apply_to_every_column_count_page_and_combined_filters() {
    for column in ["id", "session_id", "streamer_id", "job_type"] {
        let pool = pool().await;
        let repo = SqlxJobRepository::new(pool.clone(), pool.clone());
        let mut models = Vec::new();
        for (index, value) in VALUES.iter().enumerate() {
            let mut model = JobDbModel::new("execute", "{}");
            model.id = format!("job-{index}");
            model.streamer_id = Some("owner".to_owned());
            model.session_id = Some("session".to_owned());
            model.pipeline_id = Some("pipeline".to_owned());
            model.created_at = 1000 + index as i64;
            model.priority = index as i32;
            match column {
                "id" => model.id = (*value).to_owned(),
                "session_id" => model.session_id = Some((*value).to_owned()),
                "streamer_id" => model.streamer_id = Some((*value).to_owned()),
                "job_type" => model.job_type = (*value).to_owned(),
                _ => unreachable!(),
            }
            repo.create_job(&model).await.unwrap();
            models.push(model);
        }
        let ids: Vec<_> = models.iter().map(|model| model.id.clone()).collect();
        for &(search, expected) in CASES {
            let filters = JobFilters::new().with_search(search);
            let (rows, total) = repo
                .list_jobs_filtered(&filters, &Pagination::new(100, 0))
                .await
                .unwrap();
            assert_eq!(total as usize, expected.len(), "{column}: {search}");
            assert_ids(rows.iter().map(|row| row.id.clone()), &ids, expected);
            assert_eq!(repo.count_jobs(&filters).await.unwrap(), total);
            let page = repo
                .list_jobs_page_filtered(&filters, &Pagination::new(2, 1))
                .await
                .unwrap();
            assert_eq!(
                page.into_iter().map(|row| row.id).collect::<Vec<_>>(),
                rows.into_iter()
                    .skip(1)
                    .take(2)
                    .map(|row| row.id)
                    .collect::<Vec<_>>()
            );
        }
        let target = &models[2];
        let filters = JobFilters {
            status: Some(JobStatus::Pending),
            statuses: Some(vec![JobStatus::Pending, JobStatus::Failed]),
            streamer_id: target.streamer_id.clone(),
            session_id: target.session_id.clone(),
            pipeline_id: target.pipeline_id.clone(),
            from_date: DateTime::from_timestamp_millis(target.created_at),
            to_date: DateTime::from_timestamp_millis(target.created_at),
            job_type: Some(target.job_type.clone()),
            job_types: Some(vec![target.job_type.clone(), "other".to_owned()]),
            search: Some("%".to_owned()),
        };
        let (rows, total) = repo
            .list_jobs_filtered(&filters, &Pagination::new(2, 0))
            .await
            .unwrap();
        assert_eq!(total, 1);
        assert_ids(rows.into_iter().map(|row| row.id), &ids, &[2]);
        assert_eq!(repo.count_jobs(&filters).await.unwrap(), 1);
        assert_eq!(
            repo.list_jobs_page_filtered(&filters, &Pagination::new(2, 1))
                .await
                .unwrap()
                .len(),
            0
        );
        pool.close().await;
    }
}

#[tokio::test]
async fn session_search_literals_cover_live_and_retained_names_titles_ids_and_filters() {
    for column in ["name", "retained_name", "titles", "id"] {
        let pool = pool().await;
        let repo = SqlxSessionRepository::new(pool.clone(), pool.clone());
        let mut models = Vec::new();
        for (index, value) in VALUES.iter().enumerate() {
            let owner = streamer(
                &pool,
                index,
                if column == "name" { value } else { "neutral" },
            )
            .await;
            let mut model = LiveSessionDbModel::new(owner);
            model.id = if column == "id" {
                (*value).to_owned()
            } else {
                format!("session-{index}")
            };
            model.start_time = 1000 + index as i64;
            if column == "retained_name" {
                model.streamer_id = None;
                model.streamer_name = Some((*value).to_owned());
            }
            if column == "titles" {
                model.titles = Some(serde_json::json!([{"title": value, "ts": 1000}]).to_string());
            }
            repo.create_session(&model).await.unwrap();
            models.push(model);
        }
        let ids: Vec<_> = models.iter().map(|model| model.id.clone()).collect();
        for &(search, expected) in CASES {
            let filters = SessionFilters::new().with_search(search);
            let (rows, total) = repo
                .list_sessions_filtered(&filters, &Pagination::new(100, 0))
                .await
                .unwrap();
            assert_eq!(total as usize, expected.len(), "{column}: {search}");
            assert_ids(rows.iter().map(|row| row.id.clone()), &ids, expected);
            let (page, page_total) = repo
                .list_sessions_filtered(&filters, &Pagination::new(2, 1))
                .await
                .unwrap();
            assert_eq!(page_total, total);
            assert_eq!(
                page.into_iter().map(|row| row.id).collect::<Vec<_>>(),
                rows.into_iter()
                    .skip(1)
                    .take(2)
                    .map(|row| row.id)
                    .collect::<Vec<_>>()
            );
        }
        if column == "titles" {
            for (search, expected) in [
                (r#""title""#, (0..VALUES.len()).collect::<Vec<_>>()),
                (r"folder\\clip", vec![4]),
                (r"folder\clip", vec![]),
            ] {
                let (rows, total) = repo
                    .list_sessions_filtered(
                        &SessionFilters::new().with_search(search),
                        &Pagination::new(100, 0),
                    )
                    .await
                    .unwrap();
                assert_eq!(total as usize, expected.len());
                assert_ids(rows.into_iter().map(|row| row.id), &ids, &expected);
            }
        }
        let target = &models[2];
        let filters = SessionFilters {
            streamer_id: target.streamer_id.clone(),
            from_date: DateTime::from_timestamp_millis(target.start_time),
            to_date: DateTime::from_timestamp_millis(target.start_time),
            active_only: Some(true),
            include_empty: Some(true),
            search: Some("%".to_owned()),
        };
        let (rows, total) = repo
            .list_sessions_filtered(&filters, &Pagination::new(2, 0))
            .await
            .unwrap();
        assert_eq!(total, 1);
        assert_ids(rows.into_iter().map(|row| row.id), &ids, &[2]);
        pool.close().await;
    }
}

#[tokio::test]
async fn media_search_literals_keep_counts_summaries_and_joined_filters_consistent() {
    for column in ["file_path", "session_id", "file_type"] {
        let pool = pool().await;
        let repo = SqlxSessionRepository::new(pool.clone(), pool.clone());
        let mut models = Vec::new();
        for (index, value) in VALUES.iter().enumerate() {
            let owner = streamer(&pool, index, "neutral").await;
            let mut session = LiveSessionDbModel::new(owner);
            session.id = if column == "session_id" {
                (*value).to_owned()
            } else {
                format!("session-{index}")
            };
            repo.create_session(&session).await.unwrap();
            let path = if column == "file_path" {
                format!("/{value}.mp4")
            } else {
                format!("/file-{index}.mp4")
            };
            let mut output = MediaOutputDbModel::new(session.id, path, MediaFileType::Video, 10);
            if column == "file_type" {
                // Storage accepts arbitrary type labels, including values from newer writers.
                output.file_type = (*value).to_owned();
            }
            output.created_at = 1000 + index as i64;
            repo.create_media_output(&output).await.unwrap();
            models.push(output);
        }
        let ids: Vec<_> = models.iter().map(|model| model.id.clone()).collect();
        for &(search, expected) in CASES {
            let filters = OutputFilters::new().with_search(search);
            let (rows, total) = repo
                .list_outputs_filtered(&filters, &Pagination::new(100, 0))
                .await
                .unwrap();
            assert_eq!(total as usize, expected.len(), "{column}: {search}");
            assert_ids(rows.iter().map(|row| row.id.clone()), &ids, expected);
            let summary = repo.summarize_outputs_filtered(&filters).await.unwrap();
            assert_eq!(summary.iter().map(|row| row.count).sum::<u64>(), total);
            assert_eq!(
                summary.iter().map(|row| row.size_bytes).sum::<u64>(),
                total * 10
            );
            let (page, page_total) = repo
                .list_outputs_filtered(&filters, &Pagination::new(2, 1))
                .await
                .unwrap();
            assert_eq!(page_total, total);
            assert_eq!(
                page.into_iter().map(|row| row.id).collect::<Vec<_>>(),
                rows.into_iter()
                    .skip(1)
                    .take(2)
                    .map(|row| row.id)
                    .collect::<Vec<_>>()
            );
        }
        let filters = OutputFilters::new()
            .with_search("%")
            .with_streamer_id("streamer-2")
            .with_session_id(&models[2].session_id)
            .with_file_type(&models[2].file_type);
        let (rows, total) = repo
            .list_outputs_filtered(&filters, &Pagination::new(2, 0))
            .await
            .unwrap();
        assert_eq!(total, 1);
        assert_ids(rows.into_iter().map(|row| row.id), &ids, &[2]);
        let summary = repo.summarize_outputs_filtered(&filters).await.unwrap();
        assert_eq!(summary.len(), 1);
        assert_eq!(summary[0].count, 1);
        assert_eq!(summary[0].size_bytes, 10);
        let by_type = OutputFilters::new().with_search(if column == "file_type" {
            "AUDIO_EXTRACT"
        } else {
            "video"
        });
        assert_eq!(
            repo.list_outputs_filtered(&by_type, &Pagination::new(100, 0))
                .await
                .unwrap()
                .1,
            if column == "file_type" {
                1
            } else {
                VALUES.len() as u64
            }
        );
        pool.close().await;
    }
}

#[tokio::test]
async fn notification_search_literals_cover_joined_names_json_and_all_filter_combinations() {
    for column in ["name", "payload"] {
        let pool = pool().await;
        let repo = SqlxNotificationRepository::new(pool.clone(), pool.clone());
        let mut ids = Vec::new();
        for (index, value) in VALUES.iter().enumerate() {
            let owner = streamer(
                &pool,
                index,
                if column == "name" { value } else { "neutral" },
            )
            .await;
            let model = NotificationEventLogDbModel {
                id: format!("event-{index}"),
                event_type: if index % 2 == 0 { "DownloadCompleted" } else { "DownloadError" }.to_owned(),
                priority: if index % 2 == 0 { 8 } else { 2 },
                payload: serde_json::json!({"message": if column == "payload" { value } else { "neutral" }}).to_string(),
                streamer_id: Some(owner),
                created_at: 1000 + index as i64,
            };
            repo.add_event_log(&model).await.unwrap();
            ids.push(model.id);
        }
        for &(search, expected) in CASES {
            let rows = repo
                .list_event_logs(None, None, Some(search), None, 0, 100)
                .await
                .unwrap();
            assert_ids(rows.iter().map(|row| row.id.clone()), &ids, expected);
            let page = repo
                .list_event_logs(None, None, Some(search), None, 1, 2)
                .await
                .unwrap();
            assert_eq!(
                page.into_iter().map(|row| row.id).collect::<Vec<_>>(),
                rows.into_iter()
                    .skip(1)
                    .take(2)
                    .map(|row| row.id)
                    .collect::<Vec<_>>()
            );
        }
        for mask in 0..16 {
            let search = (mask & 1 != 0).then_some("%");
            let event_type = (mask & 2 != 0).then_some("DownloadCompleted");
            let owner = (mask & 4 != 0).then_some("streamer-2");
            let priority = (mask & 8 != 0).then_some("high");
            let expected: Vec<_> = (0..VALUES.len())
                .filter(|index| {
                    (search.is_none() || [2, 8].contains(index))
                        && (event_type.is_none() || index % 2 == 0)
                        && (owner.is_none() || *index == 2)
                        && (priority.is_none() || index % 2 == 0)
                })
                .collect();
            let rows = repo
                .list_event_logs(event_type, owner, search, priority, 0, 100)
                .await
                .unwrap();
            assert_ids(rows.into_iter().map(|row| row.id), &ids, &expected);
        }
        if column == "payload" {
            for (search, expected) in [
                (r#""message""#, (0..VALUES.len()).collect::<Vec<_>>()),
                (r"folder\\clip", vec![4]),
                (r"folder\clip", vec![]),
            ] {
                let rows = repo
                    .list_event_logs(None, None, Some(search), None, 0, 100)
                    .await
                    .unwrap();
                assert_ids(rows.into_iter().map(|row| row.id), &ids, &expected);
            }
        }
        // The LEFT JOIN must still admit an event whose payload matches after its owner is gone.
        sqlx::query("UPDATE notification_event_log SET streamer_id = NULL WHERE id = 'event-2'")
            .execute(&pool)
            .await
            .unwrap();
        if column == "payload" {
            let rows = repo
                .list_event_logs(None, None, Some("ratio%"), Some("8"), 0, 100)
                .await
                .unwrap();
            assert_ids(rows.into_iter().map(|row| row.id), &ids, &[2]);
        }
        pool.close().await;
    }
}

#[tokio::test]
async fn preset_search_literals_cover_both_fields_numbered_binds_and_pagination() {
    for column in ["name", "description"] {
        let pool = Arc::new(pool().await);
        // Isolate this fixture from the built-in presets installed by migrations.
        sqlx::query("DELETE FROM job_presets")
            .execute(&*pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM pipeline_presets")
            .execute(&*pool)
            .await
            .unwrap();
        let jobs = SqliteJobPresetRepository::new(pool.clone(), pool.clone());
        let pipelines = SqlitePipelinePresetRepository::new(pool.clone(), pool.clone());
        let mut ids = Vec::new();
        let mut names = Vec::new();
        for (index, value) in VALUES.iter().enumerate() {
            let name = if column == "name" {
                (*value).to_owned()
            } else {
                format!("preset-{index}")
            };
            let description = if column == "description" {
                *value
            } else {
                "neutral"
            };
            let mut job = JobPreset::new(&name, "execute", serde_json::json!({}))
                .with_category("custom")
                .with_description(description);
            job.id = format!("preset-{index}");
            jobs.create_preset(&job).await.unwrap();
            let pipeline = PipelinePreset {
                id: job.id.clone(),
                name: name.clone(),
                description: Some(description.to_owned()),
                dag_definition: Some(r#"{"steps":[]}"#.to_owned()),
                pipeline_type: Some("dag".to_owned()),
                created_at: Utc::now(),
                updated_at: Utc::now(),
            };
            pipelines.create_pipeline_preset(&pipeline).await.unwrap();
            ids.push(job.id);
            names.push(name);
        }
        for &(search, expected) in CASES {
            let filters = JobPresetFilters {
                search: Some(search.to_owned()),
                ..Default::default()
            };
            let (rows, total) = jobs
                .list_presets_filtered(&filters, &Pagination::new(100, 0))
                .await
                .unwrap();
            assert_eq!(total as usize, expected.len());
            assert_ids(rows.iter().map(|row| row.id.clone()), &ids, expected);
            let (page, page_total) = jobs
                .list_presets_filtered(&filters, &Pagination::new(2, 1))
                .await
                .unwrap();
            assert_eq!(page_total, total);
            assert_eq!(
                page.into_iter().map(|row| row.id).collect::<Vec<_>>(),
                rows.into_iter()
                    .skip(1)
                    .take(2)
                    .map(|row| row.id)
                    .collect::<Vec<_>>()
            );
            let filters = PipelinePresetFilters {
                search: Some(search.to_owned()),
            };
            let (rows, total) = pipelines
                .list_pipeline_presets_filtered(&filters, &Pagination::new(100, 0))
                .await
                .unwrap();
            assert_eq!(total as usize, expected.len());
            assert_ids(rows.iter().map(|row| row.id.clone()), &ids, expected);
            let (page, page_total) = pipelines
                .list_pipeline_presets_filtered(&filters, &Pagination::new(2, 1))
                .await
                .unwrap();
            assert_eq!(page_total, total);
            assert_eq!(
                page.into_iter().map(|row| row.id).collect::<Vec<_>>(),
                rows.into_iter()
                    .skip(1)
                    .take(2)
                    .map(|row| row.id)
                    .collect::<Vec<_>>()
            );
        }
        for mask in 0..16 {
            let filters = JobPresetFilters {
                category: (mask & 1 != 0).then(|| "custom".to_owned()),
                processor: (mask & 2 != 0).then(|| "execute".to_owned()),
                name: (mask & 4 != 0).then(|| names[2].clone()),
                search: (mask & 8 != 0).then(|| "%".to_owned()),
            };
            let expected: Vec<_> = (0..VALUES.len())
                .filter(|index| {
                    (filters.name.is_none() || *index == 2)
                        && (filters.search.is_none() || [2, 8].contains(index))
                })
                .collect();
            let (rows, total) = jobs
                .list_presets_filtered(&filters, &Pagination::new(100, 0))
                .await
                .unwrap();
            assert_eq!(total as usize, expected.len());
            assert_ids(rows.into_iter().map(|row| row.id), &ids, &expected);
        }
        pool.close().await;
    }
}
