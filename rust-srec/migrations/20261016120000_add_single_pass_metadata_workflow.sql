-- Opt in to tagging during remux instead of rewriting the complete MP4 in a
-- separate metadata step. Existing workflows and their saved copies stay intact.
-- A user-owned row with the same id or name takes precedence.
INSERT OR IGNORE INTO pipeline_presets (
    id, name, description, dag_definition, pipeline_type, created_at, updated_at
) VALUES (
    'pipeline-full-single-pass',
    'Full Processing (Single Pass)',
    'Remux to MP4 with recording title, streamer, and date, generate a thumbnail, and upload both without a separate metadata pass.',
    '{
        "name": "Full Processing (Single Pass)",
        "steps": [
            {
                "id": "remux",
                "step": {
                    "type": "inline",
                    "processor": "remux",
                    "config": {
                        "video_codec": "copy",
                        "audio_codec": "copy",
                        "format": "mp4",
                        "faststart": true,
                        "overwrite": true,
                        "include_recording_metadata": true,
                        "metadata": []
                    }
                },
                "depends_on": []
            },
            {"id": "thumbnail", "step": {"type": "preset", "name": "thumbnail"}, "depends_on": ["remux"]},
            {"id": "upload", "step": {"type": "preset", "name": "upload"}, "depends_on": ["remux", "thumbnail"]}
        ]
    }',
    'dag',
    unixepoch('now') * 1000,
    unixepoch('now') * 1000
);
