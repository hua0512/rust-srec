-- Add NVIDIA NVENC GPU-accelerated encoding presets
-- Requires NVIDIA GPU with NVENC support and CUDA hardware acceleration

-- ============================================
-- NVENC JOB PRESETS (GPU-accelerated encoding)
-- ============================================

-- Fast NVENC H.264: GPU-accelerated encoding with good quality
INSERT INTO job_presets (id, name, description, category, processor, config, created_at, updated_at) VALUES (
    'preset-nvenc-h264-fast',
    'nvenc_h264_fast',
    'Fast GPU-accelerated H.264 encoding using NVIDIA NVENC (preset: p4, CQ 23). Great speed-to-quality ratio.',
    'compression',
    'remux',
    '{"video_codec":"h264nvenc","audio_codec":"aac","audio_bitrate":"128k","preset":"p4","crf":23,"hwaccel":"cuda","format":"mp4","faststart":true,"overwrite":true}',
    unixepoch('now') * 1000,
    unixepoch('now') * 1000
);

-- High quality NVENC H.264: Best quality GPU encoding
INSERT INTO job_presets (id, name, description, category, processor, config, created_at, updated_at) VALUES (
    'preset-nvenc-h264-hq',
    'nvenc_h264_hq',
    'High quality GPU-accelerated H.264 encoding using NVIDIA NVENC (preset: p7, CQ 20). Best NVENC quality.',
    'compression',
    'remux',
    '{"video_codec":"h264nvenc","audio_codec":"aac","audio_bitrate":"192k","preset":"p7","crf":20,"hwaccel":"cuda","format":"mp4","faststart":true,"overwrite":true}',
    unixepoch('now') * 1000,
    unixepoch('now') * 1000
);

-- Fast NVENC HEVC: GPU-accelerated H.265 for smaller files
INSERT INTO job_presets (id, name, description, category, processor, config, created_at, updated_at) VALUES (
    'preset-nvenc-hevc-fast',
    'nvenc_hevc_fast',
    'Fast GPU-accelerated HEVC/H.265 encoding using NVIDIA NVENC (preset: p4, CQ 24). Smaller files than H.264.',
    'compression',
    'remux',
    '{"video_codec":"hevcnvenc","audio_codec":"aac","audio_bitrate":"128k","preset":"p4","crf":24,"hwaccel":"cuda","format":"mp4","faststart":true,"overwrite":true}',
    unixepoch('now') * 1000,
    unixepoch('now') * 1000
);

-- High quality NVENC HEVC: Best compression with GPU
INSERT INTO job_presets (id, name, description, category, processor, config, created_at, updated_at) VALUES (
    'preset-nvenc-hevc-hq',
    'nvenc_hevc_hq',
    'High quality GPU-accelerated HEVC/H.265 encoding using NVIDIA NVENC (preset: p7, CQ 21). Best compression ratio with GPU.',
    'compression',
    'remux',
    '{"video_codec":"hevcnvenc","audio_codec":"aac","audio_bitrate":"192k","preset":"p7","crf":21,"hwaccel":"cuda","format":"mp4","faststart":true,"overwrite":true}',
    unixepoch('now') * 1000,
    unixepoch('now') * 1000
);

-- NVENC Low Latency: Optimized for real-time / streaming use cases
INSERT INTO job_presets (id, name, description, category, processor, config, created_at, updated_at) VALUES (
    'preset-nvenc-h264-ll',
    'nvenc_h264_lowlatency',
    'Low-latency GPU-accelerated H.264 encoding for real-time applications. Minimal encoding delay.',
    'compression',
    'remux',
    '{"video_codec":"h264nvenc","audio_codec":"aac","audio_bitrate":"128k","preset":"ll","crf":23,"hwaccel":"cuda","format":"mp4","faststart":true,"overwrite":true}',
    unixepoch('now') * 1000,
    unixepoch('now') * 1000
);

-- Fast NVENC AV1: GPU-accelerated AV1 for next-gen compression (requires RTX 4000+)
INSERT INTO job_presets (id, name, description, category, processor, config, created_at, updated_at) VALUES (
    'preset-nvenc-av1-fast',
    'nvenc_av1_fast',
    'Fast GPU-accelerated AV1 encoding using NVIDIA NVENC (preset: p4, CQ 28). Requires RTX 4000+ series.',
    'compression',
    'remux',
    '{"video_codec":"av1nvenc","audio_codec":"aac","audio_bitrate":"128k","preset":"p4","crf":28,"hwaccel":"cuda","format":"mp4","faststart":true,"overwrite":true}',
    unixepoch('now') * 1000,
    unixepoch('now') * 1000
);

-- High quality NVENC AV1: Best compression with next-gen codec
INSERT INTO job_presets (id, name, description, category, processor, config, created_at, updated_at) VALUES (
    'preset-nvenc-av1-hq',
    'nvenc_av1_hq',
    'High quality GPU-accelerated AV1 encoding using NVIDIA NVENC (preset: p7, CQ 24). Best compression ratio, requires RTX 4000+.',
    'compression',
    'remux',
    '{"video_codec":"av1nvenc","audio_codec":"aac","audio_bitrate":"192k","preset":"p7","crf":24,"hwaccel":"cuda","format":"mp4","faststart":true,"overwrite":true}',
    unixepoch('now') * 1000,
    unixepoch('now') * 1000
);

-- ============================================
-- NVENC PIPELINE PRESETS (GPU-accelerated workflows)
-- ============================================

-- GPU Standard: NVENC Remux + Thumbnail
INSERT INTO pipeline_presets (id, name, description, dag_definition, pipeline_type, created_at, updated_at) VALUES (
    'pipeline-nvenc-standard',
    'GPU Standard',
    'GPU-accelerated post-processing: NVENC H.264 encoding and thumbnail generation.',
    '{
        "name": "GPU Standard",
        "steps": [
            {"id": "encode", "step": {"type": "preset", "name": "nvenc_h264_fast"}, "depends_on": []},
            {"id": "thumbnail", "step": {"type": "preset", "name": "thumbnail"}, "depends_on": []}
        ]
    }',
    'dag',
    unixepoch('now') * 1000,
    unixepoch('now') * 1000
);

-- GPU HQ Archive: NVENC HEVC + Thumbnail -> Upload
INSERT INTO pipeline_presets (id, name, description, dag_definition, pipeline_type, created_at, updated_at) VALUES (
    'pipeline-nvenc-hq-archive',
    'GPU HQ Archive',
    'GPU-accelerated high quality archive: NVENC HEVC encoding, thumbnail, and cloud upload.',
    '{
        "name": "GPU HQ Archive",
        "steps": [
            {"id": "encode", "step": {"type": "preset", "name": "nvenc_hevc_hq"}, "depends_on": []},
            {"id": "thumbnail", "step": {"type": "preset", "name": "thumbnail_native"}, "depends_on": []},
            {"id": "upload", "step": {"type": "preset", "name": "upload"}, "depends_on": ["encode", "thumbnail"]}
        ]
    }',
    'dag',
    unixepoch('now') * 1000,
    unixepoch('now') * 1000
);
