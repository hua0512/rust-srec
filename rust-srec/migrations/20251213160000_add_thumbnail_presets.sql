-- Add higher resolution thumbnail presets

-- Full HD thumbnail: 1280px width for modern displays and video players
INSERT OR IGNORE INTO job_presets (id, name, description, category, processor, config, created_at, updated_at) VALUES (
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
INSERT OR IGNORE INTO job_presets (id, name, description, category, processor, config, created_at, updated_at) VALUES (
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
INSERT OR IGNORE INTO job_presets (id, name, description, category, processor, config, created_at, updated_at) VALUES (
    'preset-default-thumbnail-native',
    'thumbnail_native',
    'Generate a thumbnail at native stream resolution (no scaling). Best quality, largest file size.',
    'thumbnail',
    'thumbnail',
    '{"timestamp_secs":10,"preserve_resolution":true,"quality":1}',
    datetime('now'),
    datetime('now')
);
