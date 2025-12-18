-- Migrate Sequential Pipelines to DAG Format
-- This migration converts all existing sequential pipeline_presets to DAG format
-- and deprecates the sequential 'steps' column in favor of 'dag_definition'

-- ============================================
-- SCHEMA CHANGES
-- ============================================

-- Add DAG definition column to pipeline_presets
-- This stores the full DagPipelineDefinition JSON
ALTER TABLE pipeline_presets ADD COLUMN dag_definition TEXT;

-- Add pipeline_type column to distinguish sequential (legacy) vs dag
-- Values: 'sequential' (legacy), 'dag' (new)
ALTER TABLE pipeline_presets ADD COLUMN pipeline_type TEXT NOT NULL DEFAULT 'dag';

-- ============================================
-- MIGRATE EXISTING PIPELINE PRESETS TO DAG
-- ============================================

-- Standard: Remux -> Thumbnail (can run in parallel since they both read the same input)
UPDATE pipeline_presets SET
    dag_definition = '{
        "name": "Standard",
        "steps": [
            {"id": "remux", "step": {"type": "preset", "name": "remux"}, "depends_on": []},
            {"id": "thumbnail", "step": {"type": "preset", "name": "thumbnail"}, "depends_on": []}
        ]
    }',
    pipeline_type = 'dag',
    updated_at = datetime('now')
WHERE id = 'pipeline-standard';

-- Archive to Cloud: Compress -> Upload -> Delete (sequential, each depends on previous)
UPDATE pipeline_presets SET
    dag_definition = '{
        "name": "Archive to Cloud",
        "steps": [
            {"id": "compress", "step": {"type": "preset", "name": "compress_fast"}, "depends_on": []},
            {"id": "upload", "step": {"type": "preset", "name": "upload"}, "depends_on": ["compress"]},
            {"id": "delete", "step": {"type": "preset", "name": "delete_source"}, "depends_on": ["upload"]}
        ]
    }',
    pipeline_type = 'dag',
    updated_at = datetime('now')
WHERE id = 'pipeline-archive';

-- High Quality Archive: Compress + Thumbnail (parallel) -> Upload (fan-in)
UPDATE pipeline_presets SET
    dag_definition = '{
        "name": "High Quality Archive",
        "steps": [
            {"id": "compress", "step": {"type": "preset", "name": "compress_hq"}, "depends_on": []},
            {"id": "thumbnail", "step": {"type": "preset", "name": "thumbnail_hd"}, "depends_on": []},
            {"id": "upload", "step": {"type": "preset", "name": "upload"}, "depends_on": ["compress", "thumbnail"]}
        ]
    }',
    pipeline_type = 'dag',
    updated_at = datetime('now')
WHERE id = 'pipeline-hq-archive';

-- Podcast Extraction: Audio extraction -> Upload (sequential)
UPDATE pipeline_presets SET
    dag_definition = '{
        "name": "Podcast Extraction",
        "steps": [
            {"id": "audio", "step": {"type": "preset", "name": "audio_mp3"}, "depends_on": []},
            {"id": "upload", "step": {"type": "preset", "name": "upload"}, "depends_on": ["audio"]}
        ]
    }',
    pipeline_type = 'dag',
    updated_at = datetime('now')
WHERE id = 'pipeline-podcast';

-- Quick Share: Compress + Thumbnail (parallel for speed)
UPDATE pipeline_presets SET
    dag_definition = '{
        "name": "Quick Share",
        "steps": [
            {"id": "compress", "step": {"type": "preset", "name": "compress_ultrafast"}, "depends_on": []},
            {"id": "thumbnail", "step": {"type": "preset", "name": "thumbnail"}, "depends_on": []}
        ]
    }',
    pipeline_type = 'dag',
    updated_at = datetime('now')
WHERE id = 'pipeline-quick-share';

-- Space Saver: Compress -> Delete (sequential)
UPDATE pipeline_presets SET
    dag_definition = '{
        "name": "Space Saver",
        "steps": [
            {"id": "compress", "step": {"type": "preset", "name": "compress_hevc_max"}, "depends_on": []},
            {"id": "delete", "step": {"type": "preset", "name": "delete_source"}, "depends_on": ["compress"]}
        ]
    }',
    pipeline_type = 'dag',
    updated_at = datetime('now')
WHERE id = 'pipeline-space-saver';

-- Full Processing: Remux -> (Thumbnail + Metadata parallel) -> Upload
-- Optimized: thumbnail and metadata can run in parallel after remux
UPDATE pipeline_presets SET
    dag_definition = '{
        "name": "Full Processing",
        "steps": [
            {"id": "remux", "step": {"type": "preset", "name": "remux"}, "depends_on": []},
            {"id": "thumbnail", "step": {"type": "preset", "name": "thumbnail"}, "depends_on": ["remux"]},
            {"id": "metadata", "step": {"type": "preset", "name": "add_metadata"}, "depends_on": ["remux"]},
            {"id": "upload", "step": {"type": "preset", "name": "upload"}, "depends_on": ["thumbnail", "metadata"]}
        ]
    }',
    pipeline_type = 'dag',
    updated_at = datetime('now')
WHERE id = 'pipeline-full';

-- Local Archive: Remux + Thumbnail (parallel) -> Move
UPDATE pipeline_presets SET
    dag_definition = '{
        "name": "Local Archive",
        "steps": [
            {"id": "remux", "step": {"type": "preset", "name": "remux"}, "depends_on": []},
            {"id": "thumbnail", "step": {"type": "preset", "name": "thumbnail"}, "depends_on": []},
            {"id": "move", "step": {"type": "preset", "name": "move"}, "depends_on": ["remux", "thumbnail"]}
        ]
    }',
    pipeline_type = 'dag',
    updated_at = datetime('now')
WHERE id = 'pipeline-local-archive';

-- ============================================
-- ADD NEW DAG-SPECIFIC PIPELINE PRESETS
-- ============================================

-- Diamond Pattern: Remux -> (Thumbnail + Audio parallel) -> Upload
-- Demonstrates fan-out and fan-in
INSERT INTO pipeline_presets (id, name, description, steps, dag_definition, pipeline_type, created_at, updated_at) VALUES (
    'pipeline-multimedia-archive',
    'Multimedia Archive',
    'Full multimedia processing: Remux video, extract audio and thumbnail in parallel, then upload all.',
    '[]',
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
INSERT INTO pipeline_presets (id, name, description, steps, dag_definition, pipeline_type, created_at, updated_at) VALUES (
    'pipeline-preview-gallery',
    'Preview Gallery',
    'Generate multiple preview images at different timestamps for a gallery view.',
    '[]',
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
INSERT INTO pipeline_presets (id, name, description, steps, dag_definition, pipeline_type, created_at, updated_at) VALUES (
    'pipeline-dual-format',
    'Dual Format',
    'Process video and extract podcast audio in parallel, then upload both.',
    '[]',
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

-- ============================================
-- ADD NEW JOB PRESET: Remux with cleanup
-- ============================================

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
-- ADD NEW PIPELINE PRESET: Stream Archive (Default Workflow)
-- ============================================

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
INSERT INTO pipeline_presets (id, name, description, steps, dag_definition, pipeline_type, created_at, updated_at) VALUES (
    'pipeline-stream-archive',
    'Stream Archive',
    'Default workflow: Remux to MP4 (deletes original), generate native-resolution thumbnail, upload both to cloud and delete local files.',
    '[]',
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
