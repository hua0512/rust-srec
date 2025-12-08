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
    job_history_retention_days INTEGER NOT NULL DEFAULT 30
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
    max_bitrate INTEGER,
    stream_selection_config TEXT,
    output_file_format TEXT,
    min_segment_size_bytes BIGINT,
    max_download_duration_secs BIGINT,
    max_part_size_bytes BIGINT,
    download_retry_policy TEXT,
    event_hooks TEXT
);

-- `template_config` table: Reusable configuration templates for streamers.
CREATE TABLE template_config (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL UNIQUE,
    output_folder TEXT,
    output_filename_template TEXT,
    max_bitrate INTEGER,
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
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

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
    download_retry_policy TEXT,
    danmu_sampling_config TEXT,
    consecutive_error_count INTEGER DEFAULT 0,
    disabled_until TEXT,
    avatar TEXT,
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

-- `live_sessions` table: Represents a single, continuous live stream event.
CREATE TABLE live_sessions (
    id TEXT PRIMARY KEY NOT NULL,
    streamer_id TEXT NOT NULL,
    start_time TEXT NOT NULL,
    end_time TEXT,
    titles TEXT,
    danmu_statistics_id TEXT,
    FOREIGN KEY (streamer_id) REFERENCES streamers(id) ON DELETE CASCADE,
    FOREIGN KEY (danmu_statistics_id) REFERENCES danmu_statistics(id)
);

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
    FOREIGN KEY (session_id) REFERENCES live_sessions(id)
);

-- System and Job Management
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
    next_job_type TEXT,                      -- Next job type to create on completion
    remaining_steps TEXT,                    -- Pipeline steps remaining (JSON array)
    pipeline_id TEXT                         -- Pipeline ID to group related jobs
);

CREATE TABLE job_execution_logs (
    id TEXT PRIMARY KEY,
    job_id TEXT NOT NULL,
    entry TEXT NOT NULL,
    created_at TEXT NOT NULL,
    FOREIGN KEY (job_id) REFERENCES job(id) ON DELETE CASCADE
);

CREATE TABLE engine_configuration (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    engine_type TEXT NOT NULL,
    config TEXT NOT NULL
);

CREATE TABLE upload_record (
    id TEXT PRIMARY KEY,
    media_output_id TEXT NOT NULL,
    platform TEXT NOT NULL,
    remote_path TEXT NOT NULL,
    status TEXT NOT NULL,
    metadata TEXT,
    created_at TEXT NOT NULL,
    completed_at TEXT,
    FOREIGN KEY (media_output_id) REFERENCES media_outputs(id)
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
    FOREIGN KEY (channel_id) REFERENCES notification_channel(id)
);

-- Security and Notifications

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
    FOREIGN KEY (channel_id) REFERENCES notification_channel(id)
);

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
CREATE INDEX idx_job_updated_at ON job(updated_at);
CREATE INDEX idx_job_created_at ON job(created_at);
CREATE INDEX idx_job_streamer_id ON job(streamer_id);
CREATE INDEX idx_job_session_id ON job(session_id);
CREATE INDEX idx_job_priority ON job(priority DESC);
CREATE INDEX idx_job_started_at ON job(started_at);
CREATE INDEX idx_job_completed_at ON job(completed_at);
CREATE INDEX idx_job_pipeline_id ON job(pipeline_id);

-- Index for the job_execution_logs table
CREATE INDEX idx_job_execution_logs_job_id ON job_execution_logs(job_id);

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
    job_history_retention_days
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
    'default-ffmpeg',
    0,                       -- Auto
    8,
    30
);
