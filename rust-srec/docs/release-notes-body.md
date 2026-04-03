## rust-srec v0.2.1

This release focuses on recording correctness, session lifecycle cleanup, better observability, platform compatibility updates (Douyu / Huya), Docker GPU support, and frontend stability improvements.

### Highlights
- Added split reason tracking across FLV/HLS pipelines, backend events, API responses, database session data, and the frontend session details UI
- Improved session lifecycle behavior: disabled streamers now close active sessions once offline is confirmed, and temporary-disabled backoff is now authoritative
- Cancellation is now treated as successful completion instead of a failure state
- Added NVIDIA GPU support for Docker hardware-accelerated transcoding
- Added NVIDIA NVENC encoder support (H.264, HEVC, AV1) with CUDA-aware scale filter, CQ quality control, and built-in presets
- Improved HLS stream resilience and circuit breaker isolation (per-streamer scope, configurable half-open thresholds)
- Made graceful stop timeout configurable for the downloader
- Improved download resilience: recovers from transient errors when strong progress has been made
- Updated Douyu API handling for newer request/version requirements and error-code behavior
- Updated Huya platform extractor for latest API changes
- Added danmu statistics collection and session UI visibility
- Improved frontend stability with FOUC/theme fixes, standardized loading states, and session playback UX improvements
- Improved notification quality with streamer display names, Telegram HTML fixes, and cleaned-up web push handling

### Review before upgrading
- Douyu HS CDN support was removed
- Split reason data model changed and includes a related database migration
- Session segment timestamp semantics changed (`created_at`, `completed_at`, `persisted_at`)
- HLS configuration was cleaned up across backend and frontend schema/forms
- Cancellation and temporary-disabled streamer semantics changed

### Maintenance
- Migrated frontend tooling to Vite 8 and SWC
- Updated Node.js from 22 to 24 LTS
- Reduced wasted CI time by skipping Rust CI jobs on frontend-only PRs
- Refreshed Rust, frontend, docs, and GitHub Actions dependencies
- Standardized repository text-file line endings with `.gitattributes`
