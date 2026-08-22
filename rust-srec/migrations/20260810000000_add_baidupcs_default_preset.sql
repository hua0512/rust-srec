-- Add a built-in BaiduPCS-Go upload step for pipeline workflows.
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
    'preset-default-upload-baidupcs',
    'baidupcs_upload',
    'Upload files to Baidu Netdisk using BaiduPCS-Go. Configure the account and destination before use.',
    'upload',
    'baidupcs',
    '{"destination_root":"/rust-srec/{streamer}/%Y-%m","time_anchor":"job_created","policy":"skip","norapid":false,"args":[],"max_retries":3,"remove_source_after_upload":false}',
    unixepoch('now') * 1000,
    unixepoch('now') * 1000
);
