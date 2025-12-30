-- Add default presets for danmu/subtitle processing.
--
-- These presets are intended for DAG pipelines where danmu XML is converted to ASS and then
-- burned into video outputs. Inserts are idempotent (safe to re-run on existing DBs).

-- ============================================
-- DANMU / SUBTITLE PRESETS
-- ============================================

-- Danmu XML -> ASS subtitles (DanmakuFactory)
INSERT OR IGNORE INTO job_presets (id, name, description, category, processor, config, created_at, updated_at) VALUES (
    'preset-default-danmu-to-ass',
    'danmu_to_ass',
    'Convert danmu XML (Bilibili-style) into .ass subtitles using DanmakuFactory. Manifest-aware and batch-safe.',
    'danmu',
    'danmaku_factory',
    '{"overwrite":true,"verify_output_exists":true,"prefer_manifest":true,"passthrough_inputs":true,"delete_source_xml_on_success":false}',
    datetime('now'),
    datetime('now')
);

-- Burn ASS subtitles into video frames (ffmpeg subtitles filter)
INSERT OR IGNORE INTO job_presets (id, name, description, category, processor, config, created_at, updated_at) VALUES (
    'preset-default-ass-burnin',
    'ass_burnin',
    'Burn .ass subtitles into videos (produces *_burnin.mp4 by default). Manifest-aware and batch-safe.',
    'subtitle',
    'ass_burnin',
    '{"match_strategy":"manifest","require_ass":true,"passthrough_inputs":true,"exclude_ass_from_passthrough":true,"output_extension":"mp4","video_codec":"libx264","audio_codec":"copy","crf":23,"preset":"veryfast","overwrite":true,"delete_source_videos_on_success":false,"delete_source_ass_on_success":false}',
    datetime('now'),
    datetime('now')
);

