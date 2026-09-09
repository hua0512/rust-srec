use std::sync::Arc;

use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

use super::*;
use super::{config, filter, notification, streamer, user};
use crate::database::models::*;

fn distinctive<T: Serialize + DeserializeOwned>(seed: &T, generation: i64) -> T {
    let mut value = serde_json::to_value(seed).unwrap();
    for (index, (key, field)) in value.as_object_mut().unwrap().iter_mut().enumerate() {
        let integer = generation * 100 + index as i64 + 1;
        *field = match key.as_str() {
            "id" | "deleted_at" => field.clone(),
            "platform_config_id" => json!(if generation % 2 == 0 {
                "platform-huya"
            } else {
                "platform-douyin"
            }),
            "template_config_id" => {
                json!(format!("projection-ref-template-{}", generation % 2 + 1))
            }
            "streamer_id" => json!(format!("projection-ref-streamer-{}", generation % 2 + 1)),
            "created_at" | "updated_at" | "last_login_at" | "last_live_time" | "disabled_until" => {
                let at = 1_700_000_000_000 + integer;
                if field.is_string() {
                    json!(chrono::DateTime::from_timestamp_millis(at).unwrap())
                } else {
                    json!(at)
                }
            }
            "state" => json!(if generation % 2 == 0 {
                "LIVE"
            } else {
                "NOT_LIVE"
            }),
            "priority" => json!(if generation % 2 == 0 {
                "HIGH"
            } else {
                "NORMAL"
            }),
            "engine_type" => json!(if generation % 2 == 0 {
                "FFMPEG"
            } else {
                "MESIO"
            }),
            "channel_type" => json!(if generation % 2 == 0 {
                "Email"
            } else {
                "Webhook"
            }),
            "filter_type" => json!(if generation % 2 == 0 {
                "CATEGORY"
            } else {
                "KEYWORD"
            }),
            "record_danmu"
            | "is_active"
            | "must_change_password"
            | "auto_thumbnail"
            | "stream_proxy_allow_private_targets" => json!(generation % 2 == 0),
            "fetch_delay_ms"
            | "download_delay_ms"
            | "min_segment_size_bytes"
            | "max_download_duration_secs"
            | "max_part_size_bytes"
            | "offline_check_count"
            | "offline_check_delay_ms"
            | "consecutive_error_count" => json!(integer),
            "roles" => json!(json!(["user", format!("role-{generation}")]).to_string()),
            "pipeline_type" => json!("dag"),
            _ if field.is_number() => json!(integer),
            _ if field.is_boolean() => json!(generation % 2 == 0),
            _ => json!(format!("projection-{generation}-{key}")),
        };
    }
    serde_json::from_value(value).unwrap()
}

fn expected_update(previous: Value, mut candidate: Value, imported: bool) -> Value {
    for key in ["created_at", "deleted_at"] {
        if let Some(value) = previous.get(key) {
            candidate[key] = value.clone();
        }
    }
    if imported {
        for key in ["extractor", "default_extractor"] {
            if let Some(value) = previous.get(key) {
                candidate[key] = value.clone();
            }
        }
    }
    candidate
}

#[tokio::test]
async fn complete_row_projections_match_repository_and_import_modes() {
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        let pool = crate::database::init_pool_with_size("sqlite::memory:", 1).await.unwrap();
        crate::database::run_migrations(&pool).await.unwrap();
        for number in [1, 2] {
            sqlx::query("INSERT INTO template_config(id,name) VALUES (?,?)")
                .bind(format!("projection-ref-template-{number}")).bind(format!("reference-template-{number}"))
                .execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO streamers(id,name,url,platform_config_id,state) VALUES (?,?,?,'platform-huya','NOT_LIVE')")
                .bind(format!("projection-ref-streamer-{number}")).bind(format!("reference-streamer-{number}"))
                .bind(format!("https://example.com/reference-{number}")).execute(&pool).await.unwrap();
        }
        let config_repo = SqlxConfigRepository::new(pool.clone(), pool.clone());
        let streamer_repo = SqlxStreamerRepository::new(pool.clone(), pool.clone());
        let filter_repo = SqlxFilterRepository::new(pool.clone(), pool.clone());
        let notification_repo = SqlxNotificationRepository::new(pool.clone(), pool.clone());
        let job_repo = SqliteJobPresetRepository::new(Arc::new(pool.clone()), Arc::new(pool.clone()));
        let pipeline_repo = SqlitePipelinePresetRepository::new(Arc::new(pool.clone()), Arc::new(pool.clone()));
        let user_repo = SqlxUserRepository::new(pool.clone(), pool.clone());

        macro_rules! check {
            ($ty:ident, $table:literal, $seed:expr, $repo:ident, $create:ident, $update:ident, $import:path, $clock:expr) => {{
                let seed = $ty {
                    id: concat!("projection-", $table).to_owned(),
                    ..$seed
                };
                let original = distinctive(&seed, 1);
                $repo.$create(&original).await.unwrap();
                let read_sql = format!("SELECT * FROM {} WHERE id = ?", $table);
                let actual: $ty = sqlx::query_as(sqlx::AssertSqlSafe(read_sql.clone())).bind(&seed.id).fetch_one(&pool).await.unwrap();
                assert_eq!(serde_json::to_value(&actual).unwrap(), serde_json::to_value(&original).unwrap(), "{} creation", $table);
                let mut previous = serde_json::to_value(&actual).unwrap();
                if $table == "streamers" {
                    sqlx::query("UPDATE streamers SET deleted_at = 42 WHERE id = ?").bind(&seed.id).execute(&pool).await.unwrap();
                    previous["deleted_at"] = json!(42);
                }
                let candidate = distinctive(&original, 2);
                let before = crate::database::time::now_ms();
                $repo.$update(&candidate).await.unwrap();
                let after = crate::database::time::now_ms();
                let actual: $ty = sqlx::query_as(sqlx::AssertSqlSafe(read_sql.clone())).bind(&seed.id).fetch_one(&pool).await.unwrap();
                let mut expected = expected_update(previous, serde_json::to_value(&candidate).unwrap(), false);
                let actual_json = serde_json::to_value(&actual).unwrap();
                if $clock {
                    let at = &actual_json["updated_at"];
                    let at = at.as_i64().unwrap_or_else(|| chrono::DateTime::parse_from_rfc3339(at.as_str().unwrap()).unwrap().timestamp_millis());
                    assert!((before..=after).contains(&at));
                    expected["updated_at"] = actual_json["updated_at"].clone();
                }
                assert_eq!(actual_json, expected, "{} update", $table);
                let mut imported = distinctive(&original, 3);
                let mut tx = crate::database::begin_immediate(&pool).await.unwrap();
                if $table == "filters" {
                    assert!($import(&mut tx, &imported).await.is_err(), "filter import retains insert-only conflicts");
                    imported.id.push_str("-import");
                }
                $import(&mut tx, &imported).await.unwrap();
                tx.commit().await.unwrap();
                let actual: $ty = sqlx::query_as(sqlx::AssertSqlSafe(read_sql.clone())).bind(&imported.id).fetch_one(&pool).await.unwrap();
                let expected = if $table == "filters" { serde_json::to_value(&imported).unwrap() } else { expected_update(expected, serde_json::to_value(&imported).unwrap(), true) };
                assert_eq!(serde_json::to_value(actual).unwrap(), expected, "{} import projection", $table);
                let mut reset = expected.clone();
                for (key, value) in serde_json::to_value(&seed).unwrap().as_object().unwrap() {
                    // These nullable columns have populated constructor defaults.
                    let nullable_default = matches!(key.as_str(), "consecutive_error_count" | "dag_definition");
                    if (value.is_null() || nullable_default) && key != "deleted_at" { reset[key] = Value::Null; }
                }
                let reset_model: $ty = serde_json::from_value(reset.clone()).unwrap();
                $repo.$update(&reset_model).await.unwrap();
                let actual: $ty = sqlx::query_as(sqlx::AssertSqlSafe(read_sql.clone())).bind(&reset_model.id).fetch_one(&pool).await.unwrap();
                let actual = serde_json::to_value(actual).unwrap();
                if $clock { reset["updated_at"] = actual["updated_at"].clone(); }
                assert_eq!(actual, reset, "{} nullable reset", $table);
                let mut fresh = distinctive(&seed, 4); fresh.id.push_str("-new-import");
                let mut tx = crate::database::begin_immediate(&pool).await.unwrap();
                $import(&mut tx, &fresh).await.unwrap(); tx.commit().await.unwrap();
                let created: Option<$ty> = sqlx::query_as(sqlx::AssertSqlSafe(read_sql.clone())).bind(&fresh.id).fetch_optional(&pool).await.unwrap();
                if matches!($table, "global_config" | "platform_config") {
                    assert!(created.is_none(), "import only updates existing global/platform rows");
                } else {
                    let mut expected = serde_json::to_value(&fresh).unwrap();
                    if $table == "template_config" { expected["extractor"] = Value::Null; }
                    assert_eq!(serde_json::to_value(created.unwrap()).unwrap(), expected, "{} import creation/defaults", $table);
                }
                assert!($repo.$create(&original).await.is_err(), "create must not become upsert");
                let mut missing = original.clone(); missing.id.push_str("-missing");
                $repo.$update(&missing).await.unwrap();
                assert!(sqlx::query_as::<_, $ty>(sqlx::AssertSqlSafe(read_sql)).bind(&missing.id).fetch_optional(&pool).await.unwrap().is_none(), "update must not create");
            }};
        }
        check!(GlobalConfigDbModel, "global_config", GlobalConfigDbModel::default(), config_repo, create_global_config, update_global_config, config::import_global, false);
        check!(EngineConfigurationDbModel, "engine_configuration", EngineConfigurationDbModel::new("engine", crate::database::models::engine::EngineType::Mesio, "{}"), config_repo, create_engine_config, update_engine_config, config::import_engine, false);
        check!(TemplateConfigDbModel, "template_config", TemplateConfigDbModel::new("template"), config_repo, create_template_config, update_template_config, config::import_template, true);
        let platform: PlatformConfigDbModel = sqlx::query_as("SELECT * FROM platform_config WHERE id = 'platform-huya'").fetch_one(&pool).await.unwrap();
        check!(PlatformConfigDbModel, "platform_config", platform, config_repo, create_platform_config, update_platform_config, config::import_platform, false);
        check!(StreamerDbModel, "streamers", StreamerDbModel::new("streamer", "https://example.com/source", "platform-huya"), streamer_repo, create_streamer, update_streamer, streamer::import_streamer, false);
        check!(FilterDbModel, "filters", FilterDbModel::new("projection-ref-streamer-1", FilterType::Keyword, "{}"), filter_repo, create_filter, update_filter, filter::import_filter, false);
        check!(NotificationChannelDbModel, "notification_channel", NotificationChannelDbModel::new("channel", ChannelType::Webhook, "{}"), notification_repo, create_channel, update_channel, notification::import_channel, false);
        check!(JobPreset, "job_presets", JobPreset::new("preset", "remux", json!({})), job_repo, create_preset, update_preset, preset::import_job_preset, true);
        check!(PipelinePreset, "pipeline_presets", PipelinePreset::new("pipeline", crate::database::models::job::DagPipelineDefinition::new("test", vec![])), pipeline_repo, create_pipeline_preset, update_pipeline_preset, preset::import_pipeline_preset, true);
        check!(UserDbModel, "users", UserDbModel::new("user", "unused", vec!["user".into()]), user_repo, create, update, user::import_user, true);
    }).await.expect("projection checks must finish");
}
