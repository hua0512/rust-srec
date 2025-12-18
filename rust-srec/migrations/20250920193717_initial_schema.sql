-- Initial database schema for rust-srec
-- This migration creates all tables, indexes, constraints, and seeds default data

-- ============================================
-- CONFIGURATION TABLES
-- ============================================

-- `global_config` table: A singleton table for application-wide default settings.
CREATE TABLE global_config (
    id TEXT PRIMARY KEY NOT NULL,
    output_folder TEXT NOT NULL,
    output_filename_template TEXT NOT NULL DEFAULT "{streamer}-{title}-%Y%m%d-%H%M%S",
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
    session_gap_time_secs INTEGER NOT NULL DEFAULT 3600,
    pipeline TEXT,
    log_filter_directive TEXT NOT NULL DEFAULT 'rust_srec=info,sqlx=warn,mesio_engine=info,flv=info,hls=info'
);

-- `platform_config` table: Stores settings specific to each supported streaming platform.
CREATE TABLE platform_config (
    id TEXT PRIMARY KEY NOT NULL,
    platform_name TEXT NOT NULL UNIQUE,
    fetch_delay_ms INTEGER,
    download_delay_ms INTEGER,
    cookies TEXT,
    platform_specific_config TEXT,
    proxy_config TEXT,
    record_danmu BOOLEAN,
    output_folder TEXT,
    output_filename_template TEXT,
    download_engine TEXT,
    stream_selection_config TEXT,
    output_file_format TEXT,
    min_segment_size_bytes BIGINT,
    max_download_duration_secs BIGINT,
    max_part_size_bytes BIGINT,
    download_retry_policy TEXT,
    event_hooks TEXT,
    pipeline TEXT
);

-- `template_config` table: Reusable configuration templates for streamers.
CREATE TABLE template_config (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL UNIQUE,
    output_folder TEXT,
    output_filename_template TEXT,
    cookies TEXT,
    output_file_format TEXT,
    min_segment_size_bytes INTEGER,
    max_download_duration_secs INTEGER,
    max_part_size_bytes INTEGER,
    record_danmu BOOLEAN,
    platform_overrides TEXT,
    download_retry_policy TEXT,
    danmu_sampling_config TEXT,
    download_engine TEXT,
    engines_override TEXT,
    proxy_config TEXT,
    event_hooks TEXT,
    stream_selection_config TEXT,
    pipeline TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- ============================================
-- STREAMER AND MONITORING TABLES
-- ============================================

-- `streamers` table: The central entity representing a content creator to be monitored.
-- States: NOT_LIVE, LIVE, OUT_OF_SCHEDULE, OUT_OF_SPACE, FATAL_ERROR, CANCELLED, NOT_FOUND, INSPECTING_LIVE, TEMPORAL_DISABLED
-- Priority: HIGH (VIP, never miss), NORMAL (standard), LOW (background/archive)
CREATE TABLE streamers (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    url TEXT NOT NULL UNIQUE,
    platform_config_id TEXT NOT NULL,
    template_config_id TEXT,
    state TEXT NOT NULL,
    priority TEXT NOT NULL DEFAULT 'NORMAL',
    last_live_time TEXT,
    streamer_specific_config TEXT,
    consecutive_error_count INTEGER DEFAULT 0,
    -- Last recorded error message
    last_error TEXT,
    disabled_until TEXT,
    avatar TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (platform_config_id) REFERENCES platform_config(id),
    FOREIGN KEY (template_config_id) REFERENCES template_config(id)
);

-- `filters` table: Conditions to decide whether a live stream should be recorded.
CREATE TABLE filters (
    id TEXT PRIMARY KEY NOT NULL,
    streamer_id TEXT NOT NULL,
    filter_type TEXT NOT NULL,
    config TEXT NOT NULL,
    FOREIGN KEY (streamer_id) REFERENCES streamers(id) ON DELETE CASCADE
);

-- ============================================
-- SESSION AND MEDIA TABLES
-- ============================================

-- `live_sessions` table: Represents a single, continuous live stream event.
CREATE TABLE live_sessions (
    id TEXT PRIMARY KEY NOT NULL,
    streamer_id TEXT NOT NULL,
    start_time TEXT NOT NULL,
    end_time TEXT,
    titles TEXT,
    danmu_statistics_id TEXT,
    total_size_bytes BIGINT NOT NULL DEFAULT 0,
    FOREIGN KEY (streamer_id) REFERENCES streamers(id) ON DELETE CASCADE,
    FOREIGN KEY (danmu_statistics_id) REFERENCES danmu_statistics(id)
);

-- Enforce at most one active (end_time IS NULL) session per streamer.
CREATE UNIQUE INDEX live_sessions_one_active_per_streamer
    ON live_sessions (streamer_id)
    WHERE end_time IS NULL;

-- `media_outputs` table: Represents a single file generated during a live session.
CREATE TABLE media_outputs (
    id TEXT PRIMARY KEY NOT NULL,
    session_id TEXT NOT NULL,
    parent_media_output_id TEXT,
    file_path TEXT NOT NULL,
    file_type TEXT NOT NULL,
    size_bytes INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    FOREIGN KEY (session_id) REFERENCES live_sessions(id) ON DELETE CASCADE,
    FOREIGN KEY (parent_media_output_id) REFERENCES media_outputs(id) ON DELETE SET NULL
);

-- `danmu_statistics` table: Aggregated statistics for danmu messages.
CREATE TABLE danmu_statistics (
    id TEXT PRIMARY KEY NOT NULL,
    session_id TEXT NOT NULL UNIQUE,
    total_danmus INTEGER NOT NULL,
    danmu_rate_timeseries TEXT,
    top_talkers TEXT,
    word_frequency TEXT,
    FOREIGN KEY (session_id) REFERENCES live_sessions(id) ON DELETE CASCADE
);

-- ============================================
-- MONITORING EVENT OUTBOX
-- ============================================

-- Transactional outbox for monitor events.
-- Events are inserted in the same transaction as state/session updates and
-- published asynchronously after commit.
CREATE TABLE monitor_event_outbox (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    streamer_id TEXT NOT NULL,
    event_type TEXT NOT NULL,
    payload TEXT NOT NULL,
    created_at TEXT NOT NULL,
    delivered_at TEXT,
    attempts INTEGER NOT NULL DEFAULT 0,
    last_error TEXT,
    FOREIGN KEY (streamer_id) REFERENCES streamers(id) ON DELETE CASCADE
);

-- ============================================
-- JOB SYSTEM TABLES
-- ============================================

-- Job status: PENDING, PROCESSING, COMPLETED, FAILED, INTERRUPTED
-- INTERRUPTED jobs are reset to PENDING on restart for crash recovery
CREATE TABLE job (
    id TEXT PRIMARY KEY,
    job_type TEXT NOT NULL,
    status TEXT NOT NULL,
    config TEXT NOT NULL,
    state TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    input TEXT,                              -- Input path or source for the job
    outputs TEXT,                            -- Output paths (JSON array)
    priority INTEGER NOT NULL DEFAULT 0,     -- Job priority (higher = more urgent)
    streamer_id TEXT,                        -- Associated streamer ID
    session_id TEXT,                         -- Associated session ID
    started_at TEXT,                         -- When job started processing
    completed_at TEXT,                       -- When job completed
    error TEXT,                              -- Error message if failed
    retry_count INTEGER NOT NULL DEFAULT 0,  -- Number of retry attempts
    pipeline_id TEXT,                        -- Pipeline ID to group related jobs
    execution_info TEXT,                     -- JSON blob for detailed execution logs/result
    duration_secs REAL,                      -- Processing duration in seconds (from processor)
    queue_wait_secs REAL,                    -- Time spent waiting in queue (started_at - created_at)
    -- Link jobs to their DAG step execution (if part of a DAG)
    dag_step_execution_id TEXT REFERENCES dag_step_execution(id) ON DELETE SET NULL
);

CREATE TABLE job_execution_logs (
    id TEXT PRIMARY KEY,
    job_id TEXT NOT NULL,
    entry TEXT NOT NULL,
    created_at TEXT NOT NULL,
    level TEXT,
    message TEXT,
    FOREIGN KEY (job_id) REFERENCES job(id) ON DELETE CASCADE
);

-- This is updated frequently by workers; keep it separate from `job.execution_info` to avoid
-- large JSON rewrites and reduce contention.

CREATE TABLE job_execution_progress (
    job_id TEXT PRIMARY KEY NOT NULL,
    kind TEXT NOT NULL,
    progress TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY (job_id) REFERENCES job(id) ON DELETE CASCADE
);

CREATE INDEX idx_job_execution_progress_updated_at ON job_execution_progress(updated_at);

-- Job Presets: Reusable named job configurations
-- Used in pipeline steps referencing a preset name
-- Categories: remux, compression, thumbnail, audio, archive, upload, cleanup, file_ops, custom, metadata
CREATE TABLE job_presets (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    description TEXT, -- Optional description of what this preset does
    category TEXT,    -- Category for organizing presets
    processor TEXT NOT NULL,
    config TEXT NOT NULL, -- JSON
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Pipeline Presets: Reusable pipeline configurations
-- Users can copy these to configure streamers/templates
CREATE TABLE pipeline_presets (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    description TEXT,
    -- JSON-serialized DAG pipeline definition (DagPipelineDefinition)
    dag_definition TEXT,
    pipeline_type TEXT NOT NULL DEFAULT 'dag',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- ============================================
-- DAG SYSTEM TABLES
-- ============================================

-- Tracks the overall state of a DAG pipeline execution
CREATE TABLE dag_execution (
    id TEXT PRIMARY KEY,
    -- JSON-serialized DAG pipeline definition (DagPipelineDefinition)
    dag_definition TEXT NOT NULL,
    -- Execution status: PENDING, PROCESSING, COMPLETED, FAILED, INTERRUPTED
    status TEXT NOT NULL DEFAULT 'PENDING',
    -- Associated streamer ID
    streamer_id TEXT,
    -- Associated session ID
    session_id TEXT,
    -- ISO 8601 timestamp when the DAG was created
    created_at TEXT NOT NULL,
    -- ISO 8601 timestamp when the DAG was last updated
    updated_at TEXT NOT NULL,
    -- ISO 8601 timestamp when the DAG completed (success or failure)
    completed_at TEXT,
    -- Error message if the DAG failed
    error TEXT,
    -- Total number of steps in the DAG
    total_steps INTEGER NOT NULL,
    -- Number of steps that have completed successfully
    completed_steps INTEGER NOT NULL DEFAULT 0,
    -- Number of steps that have failed
    failed_steps INTEGER NOT NULL DEFAULT 0
);

-- Tracks individual step state within a DAG execution
CREATE TABLE dag_step_execution (
    id TEXT PRIMARY KEY,
    -- Parent DAG execution ID
    dag_id TEXT NOT NULL,
    -- Step ID within the DAG definition (e.g., "remux", "upload")
    step_id TEXT NOT NULL,
    -- Associated job ID (NULL until job is created)
    job_id TEXT,
    -- Step status: BLOCKED, PENDING, PROCESSING, COMPLETED, FAILED, CANCELLED
    status TEXT NOT NULL DEFAULT 'BLOCKED',
    -- JSON array of step IDs this step depends on
    depends_on_step_ids TEXT NOT NULL DEFAULT '[]',
    -- JSON array of output paths produced by this step
    outputs TEXT,
    -- ISO 8601 timestamp when the step was created
    created_at TEXT NOT NULL,
    -- ISO 8601 timestamp when the step was last updated
    updated_at TEXT NOT NULL,
    -- Foreign key constraints
    FOREIGN KEY (dag_id) REFERENCES dag_execution(id) ON DELETE CASCADE,
    FOREIGN KEY (job_id) REFERENCES job(id) ON DELETE SET NULL,
    -- Each step_id must be unique within a DAG
    UNIQUE (dag_id, step_id)
);

-- ============================================
-- DOWNLOAD ENGINE TABLES
-- ============================================

CREATE TABLE engine_configuration (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    engine_type TEXT NOT NULL,
    config TEXT NOT NULL
);

-- ============================================
-- UPLOAD AND CLOUD TABLES
-- ============================================

CREATE TABLE upload_record (
    id TEXT PRIMARY KEY,
    media_output_id TEXT NOT NULL,
    platform TEXT NOT NULL,
    remote_path TEXT NOT NULL,
    status TEXT NOT NULL,
    metadata TEXT,
    created_at TEXT NOT NULL,
    completed_at TEXT,
    FOREIGN KEY (media_output_id) REFERENCES media_outputs(id) ON DELETE CASCADE
);

-- ============================================
-- SECURITY AND AUTHENTICATION TABLES
-- ============================================

-- Users table: Stores user accounts for authentication
CREATE TABLE users (
    id TEXT PRIMARY KEY NOT NULL,
    username TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    email TEXT UNIQUE,
    roles TEXT NOT NULL DEFAULT '["user"]',
    is_active BOOLEAN NOT NULL DEFAULT TRUE,
    must_change_password BOOLEAN NOT NULL DEFAULT TRUE,
    last_login_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- Refresh tokens table: Stores refresh tokens for JWT authentication
CREATE TABLE refresh_tokens (
    id TEXT PRIMARY KEY NOT NULL,
    user_id TEXT NOT NULL,
    token_hash TEXT NOT NULL UNIQUE,
    expires_at TEXT NOT NULL,
    created_at TEXT NOT NULL,
    revoked_at TEXT,
    device_info TEXT,
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE TABLE api_key (
    id TEXT PRIMARY KEY,
    key_hash TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    role TEXT NOT NULL,
    created_at TEXT NOT NULL
);

-- ============================================
-- NOTIFICATION TABLES
-- ============================================

CREATE TABLE notification_channel (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    channel_type TEXT NOT NULL,
    settings TEXT NOT NULL
);

CREATE TABLE notification_subscription (
    channel_id TEXT NOT NULL,
    event_name TEXT NOT NULL,
    PRIMARY KEY (channel_id, event_name),
    FOREIGN KEY (channel_id) REFERENCES notification_channel(id) ON DELETE CASCADE
);

-- Notification Dead Letter Queue: Stores notifications that failed all retry attempts
CREATE TABLE notification_dead_letter (
    id TEXT PRIMARY KEY,
    channel_id TEXT NOT NULL,
    event_name TEXT NOT NULL,
    event_payload TEXT NOT NULL,
    error_message TEXT NOT NULL,
    retry_count INTEGER NOT NULL DEFAULT 0,
    first_attempt_at TEXT NOT NULL,
    last_attempt_at TEXT NOT NULL,
    created_at TEXT NOT NULL,
    FOREIGN KEY (channel_id) REFERENCES notification_channel(id) ON DELETE CASCADE
);

-- ============================================
-- INDEXES
-- ============================================

-- Streamer indexes
CREATE INDEX idx_streamer_platform_config_id ON streamers(platform_config_id);
CREATE INDEX idx_streamer_template_config_id ON streamers(template_config_id);
CREATE INDEX idx_streamer_state ON streamers(state);
CREATE INDEX idx_streamer_priority_state ON streamers(priority, state);
CREATE INDEX idx_streamer_priority ON streamers(priority);
-- Case-insensitive unique index on URL to prevent duplicate streamers
CREATE UNIQUE INDEX idx_streamers_url_unique ON streamers (url COLLATE NOCASE);

-- Index for the filter table
CREATE INDEX idx_filter_streamer_id ON filters(streamer_id);

-- Indexes for the live_session table
CREATE INDEX idx_live_session_streamer_id ON live_sessions(streamer_id);
CREATE INDEX idx_live_session_danmu_statistics_id ON live_sessions(danmu_statistics_id);
CREATE INDEX idx_live_session_streamer_time ON live_sessions(streamer_id, start_time DESC);

-- Indexes for the media_output table
CREATE INDEX idx_media_output_session_id ON media_outputs(session_id);
CREATE INDEX idx_media_output_parent_media_output_id ON media_outputs(parent_media_output_id);
CREATE INDEX idx_media_output_file_type ON media_outputs(file_type);

-- Index for the upload_record table
CREATE INDEX idx_upload_record_media_output_id ON upload_record(media_output_id);

-- Index for the notification_subscription table
CREATE INDEX idx_notification_subscription_channel_id ON notification_subscription(channel_id);

-- Indexes for the job table
CREATE INDEX idx_job_status_type ON job(status, job_type);
-- Hot path index for atomic "claim next pending job" dequeue (ORDER BY priority DESC, created_at DESC)
CREATE INDEX idx_job_pending_claim_order ON job(status, job_type, priority DESC, created_at DESC);
CREATE INDEX idx_job_updated_at ON job(updated_at);
CREATE INDEX idx_job_created_at ON job(created_at);
CREATE INDEX idx_job_streamer_id ON job(streamer_id);
CREATE INDEX idx_job_session_id ON job(session_id);
CREATE INDEX idx_job_priority ON job(priority DESC);
CREATE INDEX idx_job_started_at ON job(started_at);
CREATE INDEX idx_job_completed_at ON job(completed_at);
CREATE INDEX idx_job_pipeline_id ON job(pipeline_id);
-- Index for efficient purge queries
CREATE INDEX idx_jobs_completed_at_status ON job(completed_at) WHERE status IN ('COMPLETED', 'FAILED');
-- Job table index for DAG reference
CREATE INDEX idx_job_dag_step ON job(dag_step_execution_id);

-- DAG execution indexes
CREATE INDEX idx_dag_execution_status ON dag_execution(status);
CREATE INDEX idx_dag_execution_session ON dag_execution(session_id);
CREATE INDEX idx_dag_execution_streamer ON dag_execution(streamer_id);
CREATE INDEX idx_dag_execution_created_at ON dag_execution(created_at);

-- DAG step execution indexes
CREATE INDEX idx_dag_step_dag_id ON dag_step_execution(dag_id);
CREATE INDEX idx_dag_step_job_id ON dag_step_execution(job_id);
CREATE INDEX idx_dag_step_status ON dag_step_execution(status);
-- Index for finding blocked steps that might be ready
CREATE INDEX idx_dag_step_dag_status ON dag_step_execution(dag_id, status);

-- Index for the job_execution_logs table
CREATE INDEX idx_job_execution_logs_job_id ON job_execution_logs(job_id);

-- Speed up paged reads ordered by created_at.
CREATE INDEX IF NOT EXISTS idx_job_execution_logs_job_id_created_at
    ON job_execution_logs(job_id, created_at);

-- Indexes for the notification_dead_letter table
CREATE INDEX idx_dead_letter_channel ON notification_dead_letter(channel_id);
CREATE INDEX idx_dead_letter_created ON notification_dead_letter(created_at);

-- Indexes for the users table
CREATE INDEX idx_users_username ON users(username);
CREATE INDEX idx_users_email ON users(email);
CREATE INDEX idx_users_is_active ON users(is_active);

-- Indexes for the refresh_tokens table
CREATE INDEX idx_refresh_tokens_user_id ON refresh_tokens(user_id);
CREATE INDEX idx_refresh_tokens_token_hash ON refresh_tokens(token_hash);
CREATE INDEX idx_refresh_tokens_expires_at ON refresh_tokens(expires_at);

-- Indexes for job_presets
CREATE INDEX idx_job_presets_name ON job_presets(name);
CREATE INDEX idx_job_presets_processor ON job_presets(processor);
CREATE INDEX idx_job_presets_category ON job_presets(category);

-- Indexes for pipeline_presets
CREATE INDEX idx_pipeline_presets_name ON pipeline_presets(name);

-- Index for monitor_event_outbox
CREATE INDEX monitor_event_outbox_undelivered
    ON monitor_event_outbox (delivered_at, id);

-- ============================================
-- DEFAULT DATA SEEDING
-- ============================================

-- Default admin user (password: admin123!)
-- Argon2id hash generated with OWASP recommended parameters: m=19456, t=2, p=1
INSERT INTO users (id, username, password_hash, email, roles, is_active, must_change_password, created_at, updated_at)
VALUES (
    'default-admin-00000000-0000-0000-0000-000000000001',
    'admin',
    '$argon2id$v=19$m=19456,t=2,p=1$K6NWuoVhfzt4UgqNyZeejQ$wK1P6/r0MM2IK+Mzk9j9PZYz9V2M3u4+eSKZBMEaNI8',
    NULL,
    '["admin", "user"]',
    TRUE,
    TRUE,
    datetime('now'),
    datetime('now')
);

-- Seed supported platforms
INSERT INTO platform_config (id, platform_name, fetch_delay_ms, download_delay_ms) VALUES
('platform-acfun', 'acfun', NULL, NULL),
('platform-bilibili', 'bilibili', NULL, NULL),
('platform-douyin', 'douyin', NULL, NULL),
('platform-douyu', 'douyu', NULL, NULL),
('platform-huya', 'huya', NULL, NULL),
('platform-pandatv', 'pandatv', NULL, NULL),
('platform-picarto', 'picarto', NULL, NULL),
('platform-redbook', 'redbook', NULL, NULL),
('platform-tiktok', 'tiktok', NULL, NULL),
('platform-twitcasting', 'twitcasting', NULL, NULL),
('platform-twitch', 'twitch', NULL, NULL),
('platform-weibo', 'weibo', NULL, NULL);

-- Seed default engines
INSERT INTO engine_configuration (id, name, engine_type, config) VALUES
('default-ffmpeg', 'default-ffmpeg', 'FFMPEG', '{"binary_path":"ffmpeg","input_args":[],"output_args":[],"timeout_secs":30,"user_agent":null}'),
('default-streamlink', 'default-streamlink', 'STREAMLINK', '{"binary_path":"streamlink","quality":"best","extra_args":[]}'),
('default-mesio', 'default-mesio', 'MESIO', '{"buffer_size":8388608,"fix_flv":true,"fix_hls":true}');

-- Seed default global configuration
INSERT INTO global_config (
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
    session_gap_time_secs,
    log_filter_directive
) VALUES (
    'global-configuration',
    './downloads',
    '{streamer}-{title}-%Y%m%d-%H%M%S',
    'flv',
    1048576,                 -- 1MB
    0,                       -- No limit
    8589934592,              -- 8GB
    FALSE,
    6,
    3,
    60000,                   -- 60s
    '',                      -- No proxy
    10000,                   -- 10s
    3,
    'default-mesio',
    0,                       -- Auto
    8,
    30,
    3600,                    -- 1 hour
    'rust_srec=info,sqlx=warn,mesio_engine=info,flv=info,hls=info'
);

-- ============================================
-- SEED DEFAULT JOB PRESETS
-- ============================================

-- ============================================
-- REMUX PRESETS (Container format conversion)
-- ============================================

-- Remux to MP4: Copy streams without re-encoding (fast, lossless)
INSERT INTO job_presets (id, name, description, category, processor, config, created_at, updated_at) VALUES (
    'preset-default-remux',
    'remux',
    'Remux to MP4 without re-encoding. Fast and lossless - just changes the container format.',
    'remux',
    'remux',
    '{"video_codec":"copy","audio_codec":"copy","format":"mp4","faststart":true,"overwrite":true}',
    datetime('now'),
    datetime('now')
);

-- Remux to MKV: Copy streams to Matroska container
INSERT INTO job_presets (id, name, description, category, processor, config, created_at, updated_at) VALUES (
    'preset-default-remux-mkv',
    'remux_mkv',
    'Remux to MKV without re-encoding. Matroska supports more codecs and features.',
    'remux',
    'remux',
    '{"video_codec":"copy","audio_codec":"copy","format":"mkv","overwrite":true}',
    datetime('now'),
    datetime('now')
);

-- ============================================
-- COMPRESSION PRESETS (Re-encoding)
-- ============================================

-- Fast H.264 compression: Good balance of speed and quality
INSERT INTO job_presets (id, name, description, category, processor, config, created_at, updated_at) VALUES (
    'preset-default-compress-fast',
    'compress_fast',
    'Fast H.264 compression (CRF 23). Good balance of speed, quality, and file size.',
    'compression',
    'remux',
    '{"video_codec":"h264","audio_codec":"aac","audio_bitrate":"128k","preset":"veryfast","crf":23,"format":"mp4","faststart":true,"overwrite":true}',
    datetime('now'),
    datetime('now')
);

-- High quality H.265/HEVC compression: Best compression ratio
INSERT INTO job_presets (id, name, description, category, processor, config, created_at, updated_at) VALUES (
    'preset-default-compress-hq',
    'compress_hq',
    'High quality H.265/HEVC compression (CRF 22). Smaller files but slower encoding.',
    'compression',
    'remux',
    '{"video_codec":"h265","audio_codec":"aac","audio_bitrate":"192k","preset":"medium","crf":22,"format":"mp4","faststart":true,"overwrite":true}',
    datetime('now'),
    datetime('now')
);

-- ============================================
-- THUMBNAIL PRESETS
-- ============================================

-- Standard thumbnail: 320px width at 10 seconds
INSERT INTO job_presets (id, name, description, category, processor, config, created_at, updated_at) VALUES (
    'preset-default-thumbnail',
    'thumbnail',
    'Generate a thumbnail image from the video at 10 seconds (320px width).',
    'thumbnail',
    'thumbnail',
    '{"timestamp_secs":10,"width":320,"quality":2}',
    datetime('now'),
    datetime('now')
);

-- HD thumbnail: 640px width for higher quality previews
INSERT INTO job_presets (id, name, description, category, processor, config, created_at, updated_at) VALUES (
    'preset-default-thumbnail-hd',
    'thumbnail_hd',
    'Generate a high-resolution thumbnail (640px width) at 10 seconds.',
    'thumbnail',
    'thumbnail',
    '{"timestamp_secs":10,"width":640,"quality":2}',
    datetime('now'),
    datetime('now')
);

-- Full HD thumbnail: 1280px width for modern displays and video players
INSERT INTO job_presets (id, name, description, category, processor, config, created_at, updated_at) VALUES (
    'preset-default-thumbnail-fullhd',
    'thumbnail_fullhd',
    'Generate a Full HD thumbnail (1280px width) for modern displays and video players.',
    'thumbnail',
    'thumbnail',
    '{"timestamp_secs":10,"width":1280,"quality":2}',
    datetime('now'),
    datetime('now')
);

-- Max quality thumbnail: 1920px width for full 1080p preservation
INSERT INTO job_presets (id, name, description, category, processor, config, created_at, updated_at) VALUES (
    'preset-default-thumbnail-max',
    'thumbnail_max',
    'Generate a maximum quality thumbnail (1920px width) preserving full 1080p detail.',
    'thumbnail',
    'thumbnail',
    '{"timestamp_secs":10,"width":1920,"quality":1}',
    datetime('now'),
    datetime('now')
);

-- Native resolution thumbnail: preserves original stream resolution
INSERT INTO job_presets (id, name, description, category, processor, config, created_at, updated_at) VALUES (
    'preset-default-thumbnail-native',
    'thumbnail_native',
    'Generate a thumbnail at native stream resolution (no scaling). Best quality, largest file size.',
    'thumbnail',
    'thumbnail',
    '{"timestamp_secs":10,"preserve_resolution":true,"quality":1}',
    datetime('now'),
    datetime('now')
);

-- ============================================
-- AUDIO EXTRACTION PRESETS
-- ============================================

-- Extract audio to MP3 (192kbps)
INSERT INTO job_presets (id, name, description, category, processor, config, created_at, updated_at) VALUES (
    'preset-default-audio-mp3',
    'audio_mp3',
    'Extract audio track to MP3 format (192kbps). Good for podcasts and music.',
    'audio',
    'audio_extract',
    '{"format":"mp3","bitrate":"192k","overwrite":true}',
    datetime('now'),
    datetime('now')
);

-- Extract audio to AAC (high quality)
INSERT INTO job_presets (id, name, description, category, processor, config, created_at, updated_at) VALUES (
    'preset-default-audio-aac',
    'audio_aac',
    'Extract audio track to AAC format (256kbps). High quality, widely compatible.',
    'audio',
    'audio_extract',
    '{"format":"aac","bitrate":"256k","overwrite":true}',
    datetime('now'),
    datetime('now')
);

-- ============================================
-- ARCHIVE PRESETS
-- ============================================

-- Create ZIP archive
INSERT INTO job_presets (id, name, description, category, processor, config, created_at, updated_at) VALUES (
    'preset-default-archive-zip',
    'archive_zip',
    'Create a ZIP archive of the file. Good for bundling with metadata.',
    'archive',
    'compression',
    '{"format":"zip","compression_level":6,"overwrite":true}',
    datetime('now'),
    datetime('now')
);

-- ============================================
-- CLEANUP PRESETS
-- ============================================

-- Delete source file
INSERT INTO job_presets (id, name, description, category, processor, config, created_at, updated_at) VALUES (
    'preset-default-delete',
    'delete_source',
    'Delete the source file. Use as the last step in a pipeline to clean up.',
    'cleanup',
    'delete',
    '{"max_retries":3,"retry_delay_ms":100}',
    datetime('now'),
    datetime('now')
);

-- ============================================
-- FILE OPERATION PRESETS
-- ============================================

-- Copy file to another location
INSERT INTO job_presets (id, name, description, category, processor, config, created_at, updated_at) VALUES (
    'preset-default-copy',
    'copy',
    'Copy the file to another location. Keeps the original file.',
    'file_ops',
    'copy_move',
    '{"operation":"copy","create_dirs":true,"verify_integrity":true,"overwrite":false}',
    datetime('now'),
    datetime('now')
);

-- Move file to another location
INSERT INTO job_presets (id, name, description, category, processor, config, created_at, updated_at) VALUES (
    'preset-default-move',
    'move',
    'Move the file to another location. Removes the original file.',
    'file_ops',
    'copy_move',
    '{"operation":"move","create_dirs":true,"verify_integrity":true,"overwrite":false}',
    datetime('now'),
    datetime('now')
);

-- ============================================
-- UPLOAD PRESETS (Cloud storage)
-- ============================================

-- Upload to cloud storage via rclone (generic)
INSERT INTO job_presets (id, name, description, category, processor, config, created_at, updated_at) VALUES (
    'preset-default-upload',
    'upload',
    'Upload file to cloud storage using rclone. Configure remote in rclone config.',
    'upload',
    'rclone',
    '{"operation":"copy","remote":"remote:","remote_path":"/uploads/{streamer}/{date}","delete_after":false,"bandwidth_limit":null}',
    datetime('now'),
    datetime('now')
);

-- Upload and delete source
INSERT INTO job_presets (id, name, description, category, processor, config, created_at, updated_at) VALUES (
    'preset-default-upload-delete',
    'upload_and_delete',
    'Upload file to cloud storage and delete local copy after successful upload.',
    'upload',
    'rclone',
    '{"operation":"move","remote":"remote:","remote_path":"/uploads/{streamer}/{date}","delete_after":true,"bandwidth_limit":null}',
    datetime('now'),
    datetime('now')
);

-- ============================================
-- METADATA PRESETS
-- ============================================

-- Add metadata tags
INSERT INTO job_presets (id, name, description, category, processor, config, created_at, updated_at) VALUES (
    'preset-default-metadata',
    'add_metadata',
    'Add metadata tags (title, artist, date) to the video file.',
    'metadata',
    'metadata',
    '{"title":"{title}","artist":"{streamer}","date":"{date}","comment":"Recorded by rust-srec"}',
    datetime('now'),
    datetime('now')
);

-- ============================================
-- CUSTOM EXECUTE PRESETS
-- ============================================

-- Custom FFmpeg command template
INSERT INTO job_presets (id, name, description, category, processor, config, created_at, updated_at) VALUES (
    'preset-default-custom-ffmpeg',
    'custom_ffmpeg',
    'Run a custom FFmpeg command. Edit the args to customize.',
    'custom',
    'execute',
    '{"command":"ffmpeg","args":["-i","{input}","-c","copy","{output}"],"timeout_secs":3600,"working_dir":null}',
    datetime('now'),
    datetime('now')
);

-- ============================================
-- ADDITIONAL PRESETS (Specialized configurations)
-- ============================================

-- Remux MP4 with faststart (optimized for streaming)
INSERT INTO job_presets (id, name, description, category, processor, config, created_at, updated_at) VALUES (
    'preset-remux-faststart',
    'remux_faststart',
    'Remux to MP4 with faststart flag for web streaming optimization.',
    'remux',
    'remux',
    '{"video_codec":"copy","audio_codec":"copy","format":"mp4","faststart":true,"overwrite":true}',
    datetime('now'),
    datetime('now')
);

-- H.264 medium compression for archival
INSERT INTO job_presets (id, name, description, category, processor, config, created_at, updated_at) VALUES (
    'preset-compress-archive',
    'compress_archive',
    'H.264 medium compression (CRF 23) optimized for long-term storage.',
    'compression',
    'remux',
    '{"video_codec":"h264","audio_codec":"aac","audio_bitrate":"128k","preset":"medium","crf":23,"format":"mp4","faststart":true,"overwrite":true}',
    datetime('now'),
    datetime('now')
);

-- High-quality MP3 audio extraction
INSERT INTO job_presets (id, name, description, category, processor, config, created_at, updated_at) VALUES (
    'preset-audio-mp3-hq',
    'audio_mp3_hq',
    'Extract audio to high-quality MP3 (320kbps) for podcast distribution.',
    'audio',
    'audio_extract',
    '{"format":"mp3","bitrate":"320k","overwrite":true}',
    datetime('now'),
    datetime('now')
);

-- Thumbnail at 30 seconds (preview)
INSERT INTO job_presets (id, name, description, category, processor, config, created_at, updated_at) VALUES (
    'preset-thumbnail-preview',
    'thumbnail_preview',
    'Generate a thumbnail at 30 seconds with 480px width for previews.',
    'thumbnail',
    'thumbnail',
    '{"timestamp_secs":30,"width":480,"quality":2}',
    datetime('now'),
    datetime('now')
);

-- HEVC maximum compression (space saver)
INSERT INTO job_presets (id, name, description, category, processor, config, created_at, updated_at) VALUES (
    'preset-compress-hevc-max',
    'compress_hevc_max',
    'Maximum HEVC/H.265 compression (CRF 28) for minimal file size.',
    'compression',
    'remux',
    '{"video_codec":"h265","audio_codec":"aac","audio_bitrate":"96k","preset":"slow","crf":28,"format":"mp4","faststart":true,"overwrite":true}',
    datetime('now'),
    datetime('now')
);

-- Ultrafast H.264 encoding (quick share)
INSERT INTO job_presets (id, name, description, category, processor, config, created_at, updated_at) VALUES (
    'preset-compress-ultrafast',
    'compress_ultrafast',
    'Ultrafast H.264 encoding (CRF 26) for quick sharing.',
    'compression',
    'remux',
    '{"video_codec":"h264","audio_codec":"aac","audio_bitrate":"128k","preset":"ultrafast","crf":26,"format":"mp4","faststart":true,"overwrite":true}',
    datetime('now'),
    datetime('now')
);

-- Remux Clean: Remux to MP4 and delete original file on success
INSERT INTO job_presets (id, name, description, category, processor, config, created_at, updated_at) VALUES (
    'preset-remux-clean',
    'remux_clean',
    'Remux to MP4 without re-encoding and delete the original file on success. Saves disk space.',
    'remux',
    'remux',
    '{"video_codec":"copy","audio_codec":"copy","format":"mp4","faststart":true,"overwrite":true,"remove_input_on_success":true}',
    datetime('now'),
    datetime('now')
);

-- ============================================
-- PIPELINE PRESETS (DAG Workflows)
-- ============================================

-- Standard: Remux -> Thumbnail (can run in parallel since they both read the same input)
INSERT INTO pipeline_presets (id, name, description, dag_definition, pipeline_type, created_at, updated_at) VALUES (
    'pipeline-standard',
    'Standard',
    'Basic post-processing: Remux FLV to MP4 and generate a thumbnail preview.',
    '{
        "name": "Standard",
        "steps": [
            {"id": "remux", "step": {"type": "preset", "name": "remux"}, "depends_on": []},
            {"id": "thumbnail", "step": {"type": "preset", "name": "thumbnail"}, "depends_on": []}
        ]
    }',
    'dag',
    datetime('now'),
    datetime('now')
);

-- Archive to Cloud: Compress -> Upload -> Delete (sequential, each depends on previous)
INSERT INTO pipeline_presets (id, name, description, dag_definition, pipeline_type, created_at, updated_at) VALUES (
    'pipeline-archive',
    'Archive to Cloud',
    'Compress video for storage, upload to cloud, then delete local file to save space.',
    '{
        "name": "Archive to Cloud",
        "steps": [
            {"id": "compress", "step": {"type": "preset", "name": "compress_fast"}, "depends_on": []},
            {"id": "upload", "step": {"type": "preset", "name": "upload"}, "depends_on": ["compress"]},
            {"id": "delete", "step": {"type": "preset", "name": "delete_source"}, "depends_on": ["upload"]}
        ]
    }',
    'dag',
    datetime('now'),
    datetime('now')
);

-- High Quality Archive: Compress + Thumbnail (parallel) -> Upload (fan-in)
INSERT INTO pipeline_presets (id, name, description, dag_definition, pipeline_type, created_at, updated_at) VALUES (
    'pipeline-hq-archive',
    'High Quality Archive',
    'Maximum quality compression with HEVC, then upload to cloud storage.',
    '{
        "name": "High Quality Archive",
        "steps": [
            {"id": "compress", "step": {"type": "preset", "name": "compress_hq"}, "depends_on": []},
            {"id": "thumbnail", "step": {"type": "preset", "name": "thumbnail_hd"}, "depends_on": []},
            {"id": "upload", "step": {"type": "preset", "name": "upload"}, "depends_on": ["compress", "thumbnail"]}
        ]
    }',
    'dag',
    datetime('now'),
    datetime('now')
);

-- Podcast Extraction: Audio extraction -> Upload (sequential)
INSERT INTO pipeline_presets (id, name, description, dag_definition, pipeline_type, created_at, updated_at) VALUES (
    'pipeline-podcast',
    'Podcast Extraction',
    'Extract high-quality audio for podcast distribution and upload.',
    '{
        "name": "Podcast Extraction",
        "steps": [
            {"id": "audio", "step": {"type": "preset", "name": "audio_mp3"}, "depends_on": []},
            {"id": "upload", "step": {"type": "preset", "name": "upload"}, "depends_on": ["audio"]}
        ]
    }',
    'dag',
    datetime('now'),
    datetime('now')
);

-- Quick Share: Compress + Thumbnail (parallel for speed)
INSERT INTO pipeline_presets (id, name, description, dag_definition, pipeline_type, created_at, updated_at) VALUES (
    'pipeline-quick-share',
    'Quick Share',
    'Fast encoding for quick sharing on social media or messaging.',
    '{
        "name": "Quick Share",
        "steps": [
            {"id": "compress", "step": {"type": "preset", "name": "compress_ultrafast"}, "depends_on": []},
            {"id": "thumbnail", "step": {"type": "preset", "name": "thumbnail"}, "depends_on": []}
        ]
    }',
    'dag',
    datetime('now'),
    datetime('now')
);

-- Space Saver: Compress -> Delete (sequential)
INSERT INTO pipeline_presets (id, name, description, dag_definition, pipeline_type, created_at, updated_at) VALUES (
    'pipeline-space-saver',
    'Space Saver',
    'Maximum compression to minimize storage usage, then delete original.',
    '{
        "name": "Space Saver",
        "steps": [
            {"id": "compress", "step": {"type": "preset", "name": "compress_hevc_max"}, "depends_on": []},
            {"id": "delete", "step": {"type": "preset", "name": "delete_source"}, "depends_on": ["compress"]}
        ]
    }',
    'dag',
    datetime('now'),
    datetime('now')
);

-- Full Processing: Remux -> (Thumbnail + Metadata parallel) -> Upload
-- Optimized: thumbnail and metadata can run in parallel after remux
INSERT INTO pipeline_presets (id, name, description, dag_definition, pipeline_type, created_at, updated_at) VALUES (
    'pipeline-full',
    'Full Processing',
    'Complete workflow: Remux, generate thumbnail, add metadata, and upload.',
    '{
        "name": "Full Processing",
        "steps": [
            {"id": "remux", "step": {"type": "preset", "name": "remux"}, "depends_on": []},
            {"id": "thumbnail", "step": {"type": "preset", "name": "thumbnail"}, "depends_on": ["remux"]},
            {"id": "metadata", "step": {"type": "preset", "name": "add_metadata"}, "depends_on": ["remux"]},
            {"id": "upload", "step": {"type": "preset", "name": "upload"}, "depends_on": ["thumbnail", "metadata"]}
        ]
    }',
    'dag',
    datetime('now'),
    datetime('now')
);

-- Local Archive: Remux + Thumbnail (parallel) -> Move
INSERT INTO pipeline_presets (id, name, description, dag_definition, pipeline_type, created_at, updated_at) VALUES (
    'pipeline-local-archive',
    'Local Archive',
    'Process locally: Remux to MP4, generate thumbnail, move to archive folder.',
    '{
        "name": "Local Archive",
        "steps": [
            {"id": "remux", "step": {"type": "preset", "name": "remux"}, "depends_on": []},
            {"id": "thumbnail", "step": {"type": "preset", "name": "thumbnail"}, "depends_on": []},
            {"id": "move", "step": {"type": "preset", "name": "move"}, "depends_on": ["remux", "thumbnail"]}
        ]
    }',
    'dag',
    datetime('now'),
    datetime('now')
);

-- ============================================
-- NEW DAG-SPECIFIC PIPELINE PRESETS
-- ============================================

-- Diamond Pattern: Remux -> (Thumbnail + Audio parallel) -> Upload
-- Demonstrates fan-out and fan-in
INSERT INTO pipeline_presets (id, name, description, dag_definition, pipeline_type, created_at, updated_at) VALUES (
    'pipeline-multimedia-archive',
    'Multimedia Archive',
    'Full multimedia processing: Remux video, extract audio and thumbnail in parallel, then upload all.',
    '{
        "name": "Multimedia Archive",
        "steps": [
            {"id": "remux", "step": {"type": "preset", "name": "remux"}, "depends_on": []},
            {"id": "thumbnail", "step": {"type": "preset", "name": "thumbnail_native"}, "depends_on": ["remux"]},
            {"id": "audio", "step": {"type": "preset", "name": "audio_aac"}, "depends_on": ["remux"]},
            {"id": "upload", "step": {"type": "preset", "name": "upload"}, "depends_on": ["remux", "thumbnail", "audio"]}
        ]
    }',
    'dag',
    datetime('now'),
    datetime('now')
);

-- Multi-Output: Generate multiple thumbnails at different timestamps
INSERT INTO pipeline_presets (id, name, description, dag_definition, pipeline_type, created_at, updated_at) VALUES (
    'pipeline-preview-gallery',
    'Preview Gallery',
    'Generate multiple preview images at different timestamps for a gallery view.',
    '{
        "name": "Preview Gallery",
        "steps": [
            {"id": "thumb_10s", "step": {"type": "inline", "processor": "thumbnail", "config": {"timestamp_secs": 10, "width": 640, "quality": 2}}, "depends_on": []},
            {"id": "thumb_30s", "step": {"type": "inline", "processor": "thumbnail", "config": {"timestamp_secs": 30, "width": 640, "quality": 2}}, "depends_on": []},
            {"id": "thumb_60s", "step": {"type": "inline", "processor": "thumbnail", "config": {"timestamp_secs": 60, "width": 640, "quality": 2}}, "depends_on": []},
            {"id": "thumb_120s", "step": {"type": "inline", "processor": "thumbnail", "config": {"timestamp_secs": 120, "width": 640, "quality": 2}}, "depends_on": []}
        ]
    }',
    'dag',
    datetime('now'),
    datetime('now')
);

-- Podcast + Video: Extract audio for podcast while also processing video
INSERT INTO pipeline_presets (id, name, description, dag_definition, pipeline_type, created_at, updated_at) VALUES (
    'pipeline-dual-format',
    'Dual Format',
    'Process video and extract podcast audio in parallel, then upload both.',
    '{
        "name": "Dual Format",
        "steps": [
            {"id": "video", "step": {"type": "preset", "name": "remux"}, "depends_on": []},
            {"id": "audio", "step": {"type": "preset", "name": "audio_mp3_hq"}, "depends_on": []},
            {"id": "thumbnail", "step": {"type": "preset", "name": "thumbnail"}, "depends_on": ["video"]},
            {"id": "upload", "step": {"type": "preset", "name": "upload"}, "depends_on": ["video", "audio", "thumbnail"]}
        ]
    }',
    'dag',
    datetime('now'),
    datetime('now')
);

-- Stream Archive: Remux (delete original) -> Thumbnail (native) -> Upload+Delete (move)
--
-- DAG Flow Explanation:
-- ====================
--
--   [INPUT: stream.flv]
--          │
--          ▼
--   ┌─────────────┐
--   │   remux     │  Step 1: Remux FLV to MP4 (copy codecs, delete original)
--   │  (root)     │  - No dependencies, starts immediately
--   │             │  - Deletes input file on success (remove_input_on_success=true)
--   │             │  - Output: video.mp4
--   └─────────────┘
--          │
--          ▼
--   ┌─────────────┐
--   │  thumbnail  │  Step 2: Generate thumbnail at native resolution
--   │  (native)   │  - Depends on remux (needs the MP4 file)
--   │             │  - Output: video.jpg
--   └─────────────┘
--          │
--          ▼
--   ┌─────────────┐
--   │   upload    │  Step 3: Upload BOTH files and delete local (fan-in)
--   │  (move)     │  - Uses rclone "move" operation (upload + delete in one step)
--   │             │  - Receives outputs from both: [video.mp4, video.jpg]
--   │             │  - After upload: local files automatically deleted by rclone
--   └─────────────┘
--          │
--          ▼
--     [COMPLETE]
--
-- Why use rclone "move" instead of "copy" + separate "cleanup"?
-- =============================================================
--
-- Option A: upload (copy) -> cleanup (delete)
--   - Two separate steps
--   - If cleanup fails, files remain locally (wasted space)
--   - If system crashes between upload and cleanup, files orphaned
--   - More jobs to track and manage
--
-- Option B: upload_and_delete (move) [RECOMMENDED]
--   - Single atomic operation
--   - rclone only deletes AFTER successful upload verification
--   - No orphaned files on crash (rclone handles this)
--   - Fewer jobs, simpler DAG
--
-- IMPORTANT: Do NOT add a "cleanup" step after "upload_and_delete"!
--   - The files are already deleted by rclone move
--   - A cleanup step would FAIL with "file not found"
--
-- Execution Flow Simulation:
-- ==========================
--
-- T=0: Pipeline created with input "stream.flv"
--      - DAG scheduler analyzes dependencies
--      - "remux" has no dependencies -> READY
--      - "thumbnail" depends on remux -> BLOCKED
--      - "upload" depends on remux, thumbnail -> BLOCKED
--
-- T=0: Job "remux" created and enqueued (status: PENDING)
--      - Worker picks up job
--      - Remuxes stream.flv -> stream.mp4
--      - Deletes stream.flv (remove_input_on_success=true)
--      - Job completes (status: COMPLETED)
--      - Output: ["stream.mp4"]
--
-- T=1: DAG scheduler notified of "remux" completion
--      - Checks dependents: "thumbnail" now has all deps satisfied -> READY
--      - "upload" still waiting for thumbnail -> BLOCKED
--
-- T=1: Job "thumbnail" created and enqueued
--      - Input: ["stream.mp4"] (from remux output)
--      - Worker picks up job
--      - Generates stream.jpg at native resolution
--      - Job completes (status: COMPLETED)
--      - Output: ["stream.jpg"]
--
-- T=2: DAG scheduler notified of "thumbnail" completion
--      - Checks dependents: "upload" now has all deps satisfied -> READY
--
-- T=2: Job "upload" created and enqueued
--      - Input: ["stream.mp4", "stream.jpg"] (merged from remux + thumbnail)
--      - Worker picks up job
--      - rclone MOVE: uploads stream.mp4 to cloud, then deletes local
--      - rclone MOVE: uploads stream.jpg to cloud, then deletes local
--      - Job completes (status: COMPLETED)
--      - Output: ["remote:path/stream.mp4", "remote:path/stream.jpg"]
--      - Local files: DELETED (by rclone, not a separate step)
--
-- T=3: DAG scheduler notified of "upload" completion
--      - No more dependents
--      - All steps completed -> DAG status: COMPLETED
--
-- Final state:
--   - Local: stream.flv (DELETED by remux)
--   - Local: stream.mp4 (DELETED by rclone move)
--   - Local: stream.jpg (DELETED by rclone move)
--   - Cloud: remote:path/stream.mp4 (uploaded)
--   - Cloud: remote:path/stream.jpg (uploaded)
--
-- Benefits of this DAG structure:
-- 1. Fan-in: Upload receives outputs from both remux and thumbnail
-- 2. Atomic cleanup: rclone move = upload + delete in one operation
-- 3. Native quality: Thumbnail preserves original video resolution
-- 4. Fail-fast: If any step fails, downstream steps don't start
-- 5. Complete archive: Both video and thumbnail uploaded together
-- 6. No orphaned files: rclone handles upload verification before delete
--
INSERT INTO pipeline_presets (id, name, description, dag_definition, pipeline_type, created_at, updated_at) VALUES (
    'pipeline-stream-archive',
    'Stream Archive',
    'Default workflow: Remux to MP4 (deletes original), generate native-resolution thumbnail, upload both to cloud and delete local files.',
    '{
        "name": "Stream Archive",
        "steps": [
            {"id": "remux", "step": {"type": "preset", "name": "remux_clean"}, "depends_on": []},
            {"id": "thumbnail", "step": {"type": "preset", "name": "thumbnail_native"}, "depends_on": ["remux"]},
            {"id": "upload", "step": {"type": "preset", "name": "upload_and_delete"}, "depends_on": ["remux", "thumbnail"]}
        ]
    }',
    'dag',
    datetime('now'),
    datetime('now')
);
