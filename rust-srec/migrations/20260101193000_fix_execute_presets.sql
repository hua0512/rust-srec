-- Fix execute job presets to match the current ExecuteCommandProcessor config format.
--
-- - Adds a generic `execute` preset if missing.
-- - Updates the legacy `custom_ffmpeg` preset if it still uses the old args-based schema.

INSERT OR IGNORE INTO job_presets (id, name, description, category, processor, config, created_at, updated_at)
VALUES (
    'preset-default-execute',
    'execute',
    'Run a custom shell command with placeholders (e.g. {input}, {inputs_json}, {streamer}, %Y%m%d).',
    'custom',
    'execute',
    '{"command":"echo {input}"}',
    datetime('now'),
    datetime('now')
);

UPDATE job_presets
SET
    description = 'Run a custom FFmpeg command. Requires explicit outputs (for {output}) or configure scan_output_dir.',
    config = '{"command":"ffmpeg -i \\"{input}\\" -c copy \\"{output}\\""}',
    updated_at = datetime('now')
WHERE id = 'preset-default-custom-ffmpeg'
  AND config LIKE '%"args"%';

