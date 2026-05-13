## rust-srec v0.3.1

This update continues the recording-session reliability work from **v0.3.0** — fewer false session endings on transient download failures, cleaner schedule-end transitions, and quieter out-of-schedule checks — and adds **GPU health monitoring** to the System Health page. The `/api/health` endpoint is also significantly faster and lighter, so opening the System Health page stays instant even on busy systems.

### Highlights
- Added **GPU health monitoring** on the System Health page — a lost GPU (a known issue with the NVIDIA Container Toolkit on cgroup v2 hosts) now triggers an immediate notification instead of waiting for the next failed remux job. The probe interval is configurable from the global settings page.
- Made **`/api/health` much faster and lighter** — the dashboard's health endpoint now reads a cached snapshot rather than re-running every check on each poll, so opening the System Health page is instant and background CPU stays low even on busy systems.
- **Fewer false session endings** when a download hits a temporary failure — rust-srec now keeps better track of which engine and stream type reported the failure, so transient network trouble is less likely to split a recording into multiple sessions.
- **Recording schedules end sessions more cleanly** — when a stream leaves its allowed schedule window, the active session is now closed through the normal session flow, keeping the dashboard, notifications, post-processing, and timeline aligned.
- **Quieter out-of-schedule checks** and a fix for the **queue freshness threshold** not persisting after settings updates.

### Review before upgrading
- One new database migration runs automatically on startup (adds the GPU health probe interval to global config).
- The old **session gap time** setting is removed from the configuration UI and ignored on import. Backups from earlier versions continue to import successfully — no manual cleanup is needed.

See the [v0.3.1 release notes](https://docs.srec.rs/en/release-notes/v0.3.1) for the full list and the Chinese version at [/zh/release-notes/v0.3.1](https://docs.srec.rs/zh/release-notes/v0.3.1).
