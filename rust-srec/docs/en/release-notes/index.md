# Release Notes

Track version updates, behavior changes, compatibility notes, and upgrade guidance for rust-srec.

## Unreleased

- [`unreleased`](./unreleased.md) — Mesio HLS reliability and media-processing improvements, unified HLS/FLV download sessions, Douyu updates, and automatic cleanup of unused Mesio settings

## Latest release

- [`v0.3.2`](./v0.3.2.md) — pipeline & recording reliability: end-of-session and paired post-processing wait for the files they need, resumed recordings keep consistent segment numbering, **Delete Source** no longer removes a converted video, and a temporary CDN failure no longer flips a live streamer offline

## Archive

- [`v0.3.1`](./v0.3.1.md) — recording-session reliability follow-ups (fewer false endings on transient failures, cleaner schedule-end transitions, quieter out-of-schedule checks) plus GPU health monitoring on the System Health page and a faster, lighter `/api/health`

- [`v0.3.0`](./v0.3.0.md) — output-root write gate (#508), session lifecycle overhaul, priority-aware queue with new Queued badge, streamer check-history strip, rclone bandwidth & throughput controls, backend notification localization, Mesio `--disable-log-file`, and platform fixes for Huya and RedBook

- [`v0.2.1`](./v0.2.1.md) — recording correctness, session lifecycle fixes, Douyu updates, danmu statistics, and frontend stability improvements

Older releases will be listed here as new versions are published.
