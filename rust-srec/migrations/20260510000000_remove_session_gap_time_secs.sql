-- Remove the inactive session_gap_time_secs setting.
--
-- SQLite cannot drop columns portably across all supported deployments, so
-- rebuild global_config with the current schema minus session_gap_time_secs.

CREATE TABLE global_config_new (
    id TEXT PRIMARY KEY NOT NULL,
    output_folder TEXT NOT NULL,
    output_filename_template TEXT NOT NULL DEFAULT "{streamer}-%Y%m%d-%H%M%S-{title}",
    output_file_format TEXT NOT NULL DEFAULT "flv",
    min_segment_size_bytes INTEGER NOT NULL DEFAULT 1048576,
    max_download_duration_secs INTEGER NOT NULL DEFAULT 0,
    max_part_size_bytes BIGINT NOT NULL DEFAULT 8589934592,
    record_danmu BOOLEAN NOT NULL DEFAULT FALSE,
    max_concurrent_downloads INTEGER NOT NULL DEFAULT 6,
    max_concurrent_uploads INTEGER NOT NULL DEFAULT 3,
    streamer_check_delay_ms INTEGER NOT NULL DEFAULT 60000,
    proxy_config TEXT NOT NULL,
    offline_check_delay_ms INTEGER NOT NULL DEFAULT 20000,
    offline_check_count INTEGER NOT NULL DEFAULT 3,
    default_download_engine TEXT NOT NULL,
    max_concurrent_cpu_jobs INTEGER NOT NULL DEFAULT 0,
    max_concurrent_io_jobs INTEGER NOT NULL DEFAULT 8,
    job_history_retention_days INTEGER NOT NULL DEFAULT 30,
    notification_event_log_retention_days INTEGER NOT NULL DEFAULT 30,
    pipeline TEXT,
    log_filter_directive TEXT NOT NULL DEFAULT 'rust_srec=info,sqlx=warn,mesio_engine=info,flv=info,hls=info',
    session_complete_pipeline TEXT,
    paired_segment_pipeline TEXT,
    auto_thumbnail BOOLEAN NOT NULL DEFAULT TRUE,
    pipeline_cpu_job_timeout_secs INTEGER NOT NULL DEFAULT 3600,
    pipeline_io_job_timeout_secs INTEGER NOT NULL DEFAULT 3600,
    pipeline_execute_timeout_secs INTEGER NOT NULL DEFAULT 3600,
    queue_freshness_threshold_ms INTEGER NOT NULL DEFAULT 60000
);

INSERT INTO global_config_new (
    id,
    output_folder,
    output_filename_template,
    output_file_format,
    min_segment_size_bytes,
    max_download_duration_secs,
    max_part_size_bytes,
    record_danmu,
    max_concurrent_downloads,
    max_concurrent_uploads,
    streamer_check_delay_ms,
    proxy_config,
    offline_check_delay_ms,
    offline_check_count,
    default_download_engine,
    max_concurrent_cpu_jobs,
    max_concurrent_io_jobs,
    job_history_retention_days,
    notification_event_log_retention_days,
    pipeline,
    log_filter_directive,
    session_complete_pipeline,
    paired_segment_pipeline,
    auto_thumbnail,
    pipeline_cpu_job_timeout_secs,
    pipeline_io_job_timeout_secs,
    pipeline_execute_timeout_secs,
    queue_freshness_threshold_ms
)
SELECT
    id,
    output_folder,
    output_filename_template,
    output_file_format,
    min_segment_size_bytes,
    max_download_duration_secs,
    max_part_size_bytes,
    record_danmu,
    max_concurrent_downloads,
    max_concurrent_uploads,
    streamer_check_delay_ms,
    proxy_config,
    offline_check_delay_ms,
    offline_check_count,
    default_download_engine,
    max_concurrent_cpu_jobs,
    max_concurrent_io_jobs,
    job_history_retention_days,
    notification_event_log_retention_days,
    pipeline,
    log_filter_directive,
    session_complete_pipeline,
    paired_segment_pipeline,
    auto_thumbnail,
    pipeline_cpu_job_timeout_secs,
    pipeline_io_job_timeout_secs,
    pipeline_execute_timeout_secs,
    queue_freshness_threshold_ms
FROM global_config;

DROP TABLE global_config;
ALTER TABLE global_config_new RENAME TO global_config;

CREATE TEMP TABLE global_config_foreign_key_check_guard (
    violation INTEGER NOT NULL CHECK (violation = 0)
);

INSERT INTO global_config_foreign_key_check_guard (violation)
SELECT 1
FROM pragma_foreign_key_check;

DROP TABLE global_config_foreign_key_check_guard;
