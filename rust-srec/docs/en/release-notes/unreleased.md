# Release Notes

## `unreleased`

### Pipeline reliability

- **Resumed recordings keep their segment numbering**

  When a recording resumes after a brief interruption, new segments now continue numbering from where the previous attempt left off instead of restarting at `0`. Thumbnails, paired danmaku, the segment list on the session page, notifications, and post-processing all stay aligned across the resume — previously, the restart could cause new segments to be mistaken for duplicates.

- **Session Complete Pipeline waits for the final recording**

  At the end of a stream, the Session Complete Pipeline now waits until the final video file has been saved and all per-segment processing has finished before it runs. Previously, if the danmaku side finished first, end-of-session steps like merging, uploading, or sending the completion notification could start too early with no video files available.

- **Paired Segment Pipeline matches files more reliably**

  Paired post-processing now waits until both the video and the danmaku for the same segment are actually ready before it starts, and the same paired job is no longer triggered twice for one segment.

### Recording reliability

- **Streamer no longer flips to offline after a temporary CDN failure**

  Fixed a case where a streamer would appear offline on the web UI (and stop recording) after a temporary CDN failure, such as an HTTP 404 on a signed playback URL. The live state is now restored as soon as the recorder resumes, and the resumed download is no longer aborted because of an outdated cached status.

### Maintenance

- Internal pipeline coordination was reorganized to make these reliability improvements easier to maintain going forward. Existing pipeline settings and presets continue to work without changes.
