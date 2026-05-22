# Release Notes

Track version updates, behavior changes, compatibility notes, and upgrade guidance for rust-srec.

## Unreleased

- [`unreleased`](./unreleased.md) — resumed recordings keep consistent segment numbering, end-of-session and paired video + danmaku post-processing wait for the right files before running, and a temporary CDN failure no longer flips a streamer to offline

## Latest release

- [`v0.3.1`](./v0.3.1.md) — recording-session reliability follow-ups (fewer false endings on transient failures, cleaner schedule-end transitions, quieter out-of-schedule checks) plus GPU health monitoring on the System Health page and a faster, lighter `/api/health`

## Archive


- [`v0.3.0`](./v0.3.0.md) — output-root write gate (#508), session lifecycle overhaul, priority-aware queue with new Queued badge, streamer check-history strip, rclone bandwidth & throughput controls, backend notification localization, Mesio `--disable-log-file`, and platform fixes for Huya and RedBook

- [`v0.2.1`](./v0.2.1.md) — recording correctness, session lifecycle fixes, Douyu updates, danmu statistics, and frontend stability improvements

Older releases will be listed here as new versions are published.
