## rust-srec v0.4.0

A feature release: two new platforms, a rebuilt HLS recording engine, bulk streamer management, automatic database cleanup, and a flash-free theme system, along with a range of recording, playback, and pipeline-editor fixes.

### Highlights
- **Two new platforms: SOOP and Bigo Live** — add SOOP (formerly AfreecaTV) and Bigo Live rooms and record them, including password-protected and login-required rooms, with live chat and gifts when danmaku is enabled.
- **Rebuilt HLS recording engine** — steadier, more predictable recording with memory kept under control, more reliable encrypted streams, and no re-downloading the same data when a stream rotates its access links (as Twitch does).
- **Bulk actions and filters for streamers** — select several streamers and enable, disable, assign a template, set priority, or delete them at once, and filter or sort large streamer lists.
- **Automatic database maintenance** — the app now cleans up its own database on a schedule so it doesn't keep growing over time; a retention setting of `0` now means "keep forever."
- **Flash-free theming** — dark mode and custom themes now apply before the first frame on both web and desktop, and the desktop app no longer flashes white on launch.
- **Smoother recorded playback** — skipping through a recording (including in fullscreen) no longer freezes the video, and recorded danmaku shows the correct times, sender names, gifts, and super chats.
- **Pipeline editor improvements** — replace a step without rebuilding its connections, a clearer graph layout, and manually placed steps that stay where you put them.

### Review before upgrading
- A database migration removes several Mesio engine settings that no longer affected recording (the whole **Performance** tab plus a few advanced fields). Your saved settings are updated automatically — no action needed.
- Database housekeeping now runs on its own: a quick pass at startup and every 30 minutes, with heavier work kept to your maintenance window. Set **Pipeline History Retention** or **Notification Log Retention** to `0` to keep that history indefinitely.

Full release notes: https://docs.srec.rs/en/release-notes/v0.4.0 · 中文版：https://docs.srec.rs/zh/release-notes/v0.4.0
