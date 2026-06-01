## rust-srec v0.3.2

This update is focused on pipeline and recording reliability — end-of-session and paired post-processing now wait for the files they need before running, resumed recordings keep their segment numbering, the **Delete Source** step no longer removes a freshly converted video, and a temporary CDN failure no longer flips a live streamer to offline.

### Highlights
- **Resumed recordings keep their segment numbering** — when a recording resumes after a brief interruption, new segments continue numbering from where the previous attempt left off instead of restarting at `0`, keeping thumbnails, paired danmaku, the segment list, notifications, and post-processing aligned.
- **Session Complete Pipeline waits for the final recording** — end-of-session steps like merging, uploading, or the completion notification now wait until the final video file is saved and all per-segment processing has finished, instead of starting too early when the danmaku side finishes first.
- **Paired Segment Pipeline matches files more reliably** — paired post-processing waits until both the video and the danmaku for the same segment are ready before it starts, and the same paired job is no longer triggered twice.
- **"Delete Source" no longer deletes your converted video** — a **Delete Source** step after a convert/transcode step was removing the newly converted file instead of the original, since a delete step always acts on the output of the step before it. The built-in **Space Saver** workflow is fixed, and the pipeline editor now warns when a delete step is placed there; use **Remove Input on Success** on the convert step to delete the original instead. Deleting after an **Upload** step is unaffected and still safe.
- **Streamer no longer flips to offline after a temporary CDN failure** — fixed a case where a streamer would appear offline (and stop recording) after a transient CDN failure such as an HTTP 404 on a signed playback URL; the live state is now restored as soon as the recorder resumes.

### Review before upgrading
- Internal pipeline coordination was reorganized to make these reliability improvements easier to maintain. Existing pipeline settings and presets continue to work without changes.
- Dependency and build updates: `sqlx` 0.8.6 → 0.9.0, `rust-i18n` 3 → 4, and `rquickjs` 0.11.0 → 0.12.0, plus the web frontend moving to react-day-picker v10 with bundle optimizations. None of these change how rust-srec behaves for you.

See the [v0.3.2 release notes](https://docs.srec.rs/en/release-notes/v0.3.2) for the full list and the Chinese version at [/zh/release-notes/v0.3.2](https://docs.srec.rs/zh/release-notes/v0.3.2).
