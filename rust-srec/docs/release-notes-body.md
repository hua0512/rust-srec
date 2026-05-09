## rust-srec v0.3.0

This release covers four independent themes: (1) **recording session reliability** — quiet periods for short network blips, automatic cleanup of empty session cards, and a per-streamer override for offline detection; (2) the **output-root write gate** — a fix for the class of failures where rust-srec could not recover from a filesystem issue (disk full, stale Docker bind mount) without a container restart; (3) a **new check-history strip** on the streamer details page; (4) **better behavior under concurrency saturation** — Queued badges, smarter ordering for high-priority streamers, and no more "everything froze" stalls. Also ships **rclone bandwidth & throughput controls**, the foundation of **backend notification localization** (en + zh-CN), a Mesio CLI flag to skip log-file creation, and platform fixes for Huya and RedBook.

### Highlights
- Added **output-root write gate** for recording filesystem failure resilience (#508) — pauses recordings at the filesystem boundary, exposes status in `/health`, sends one critical notification, and auto-recovers on disk-full once the user frees space
- Added a **check-history strip** on the streamer details page — last 60 monitor polls visualised as colored bars, with tooltips that show the chosen stream quality (#546)
- Reworked **session lifecycle**: quiet period for brief disconnects, HLS `#EXT-X-ENDLIST` closes the session immediately, ghost zero-byte sessions are cleaned up by API filter and background janitor (#534)
- Added **Queued** badge plus priority-aware queue scheduling and runtime-tunable URL freshness threshold for queued streamers (#548)
- Added **rclone bandwidth & throughput controls** — bandwidth limit, transfers, checkers, TPS limit / burst, multi-thread streams, and cutoff exposed as form fields (#547)
- Added **backend notification localization** via `rust-i18n` and a new `output_path_inaccessible` notification event
- Added Mesio `--disable-log-file` for one-off runs and ANSI color detection for redirected log output

### Platform fixes
- Huya: rectification notice no longer marks streamers as banned (#557)
- Huya: corrected CDN priority mapping; preferred / blacklisted CDN settings now apply as expected (#513 / #514)
- RedBook: ended streams now transition to offline cleanly (#510)
- Frontend: dedicated **Account Not Found** badge for `NOT_FOUND` streamers (#519)

### Review before upgrading
- Two new database migrations run automatically on startup (check-history strip + queue refresh threshold)
- `GET /sessions` no longer returns zero-byte ended sessions by default — pass `?include_empty=true` to get the previous behavior
- New env vars: `RUST_SREC_OUTPUT_ROOTS` (output-root boundaries for the write gate) and `RUST_SREC_LOCALE` (notification language: `en` or `zh-CN`)
- Internal monitor service rename: `set_circuit_breaker_blocked` → `set_infra_blocked(reason)`
- WebSocket event stream gains `DOWNLOAD_QUEUED` / `DOWNLOAD_DEQUEUED` events and a `queued` array in the initial snapshot; `DownloadRejected` now carries a `kind` discriminator

See the [v0.3.0 release notes](https://github.com/hua0512/rust-srec/blob/main/rust-srec/docs/en/release-notes/v0.3.0.md) for the full list and the Chinese version at [/zh/release-notes/v0.3.0](https://github.com/hua0512/rust-srec/blob/main/rust-srec/docs/zh/release-notes/v0.3.0.md).
