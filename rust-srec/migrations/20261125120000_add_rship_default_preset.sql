-- Add a built-in rship upload step for pipeline workflows.
INSERT OR IGNORE INTO job_presets (
    id,
    name,
    description,
    category,
    processor,
    config,
    created_at,
    updated_at
) VALUES (
    'preset-default-upload-rship',
    'rship_upload',
    'Upload files to cloud storage using rship. Configure the remote and destination before use.',
    'upload',
    'rship',
    '{"destination_root":"gdrive:{streamer}/%Y-%m","operation":"copy","time_anchor":"job_created","max_retries":3,"adaptive":false,"drive_stop_on_upload_limit":false,"checksum":false,"strict_source":false,"args":[]}',
    unixepoch('now') * 1000,
    unixepoch('now') * 1000
);
