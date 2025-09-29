
-- `global_config` table: A singleton table for application-wide default settings.
CREATE TABLE global_config (
    id TEXT PRIMARY KEY NOT NULL,
    output_folder TEXT NOT NULL,
    output_filename_template TEXT NOT NULL DEFAULT "{streamer}-{title}-{%Y%m%d-%H%M%S}",
    output_file_format TEXT NOT NULL DEFAULT "flv",
    min_segment_size_bytes INTEGER NOT NULL DEFAULT 1048576,
    max_download_duration_secs INTEGER NOT NULL DEFAULT 0,
    max_part_size_bytes INTEGER NOT NULL DEFAULT 8589934592,
    record_danmu BOOLEAN NOT NULL DEFAULT FALSE,
    max_concurrent_downloads INTEGER NOT NULL DEFAULT 6,
    max_concurrent_uploads INTEGER NOT NULL DEFAULT 3,
    streamer_check_delay_ms INTEGER NOT NULL DEFAULT 60000,
    proxy_config TEXT NOT NULL,
    offline_check_delay_ms INTEGER NOT NULL DEFAULT 20000,
    offline_check_count INTEGER NOT NULL DEFAULT 3,
    default_download_engine TEXT NOT NULL
);

-- `platform_config` table: Stores settings specific to each supported streaming platform.
CREATE TABLE platform_config (
    id TEXT PRIMARY KEY NOT NULL,
    platform_name TEXT NOT NULL UNIQUE,
    fetch_delay_ms INTEGER NOT NULL,
    download_delay_ms INTEGER NOT NULL,
    cookies TEXT,
    platform_specific_config TEXT,
    proxy_config TEXT,
    record_danmu BOOLEAN
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
    event_hooks TEXT
);

-- `streamers` table: The central entity representing a content creator to be monitored.
CREATE TABLE streamers (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    url TEXT NOT NULL UNIQUE,
    platform_config_id TEXT NOT NULL,
    template_config_id TEXT,
    state TEXT NOT NULL,
    last_live_time TEXT,
    streamer_specific_config TEXT,
    download_retry_policy TEXT,
    danmu_sampling_config TEXT,
    consecutive_error_count INTEGER,
    disabled_until TEXT,
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
    FOREIGN KEY (streamer_id) REFERENCES streamers(id) ON DELETE CASCADE
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
CREATE TABLE job (
    id TEXT PRIMARY KEY,
    job_type TEXT NOT NULL,
    status TEXT NOT NULL,
    context TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
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

-- Security and Notifications
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

CREATE INDEX idx_streamer_platform_config_id ON streamers(platform_config_id);
CREATE INDEX idx_streamer_template_config_id ON streamers(template_config_id);
CREATE INDEX idx_streamer_state ON streamers(state);

-- Index for the filter table
CREATE INDEX idx_filter_streamer_id ON filters(streamer_id);

-- Index for the live_session table
CREATE INDEX idx_live_session_streamer_id ON live_sessions(streamer_id);

-- Indexes for the media_output table
CREATE INDEX idx_media_output_session_id ON media_outputs(session_id);
CREATE INDEX idx_media_output_parent_media_output_id ON media_outputs(parent_media_output_id);

-- Index for the upload_record table
CREATE INDEX idx_upload_record_media_output_id ON upload_record(media_output_id);

-- Index for the notification_subscription table
CREATE INDEX idx_notification_subscription_channel_id ON notification_subscription(channel_id);