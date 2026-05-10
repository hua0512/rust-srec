# Release Notes

## `unreleased`

This update continues the recording-session reliability work from `v0.3.0`,
with fixes for a few cases where rust-srec could end a session too early or
leave the dashboard/timeline slightly out of sync with what was really
happening.

## Recording reliability

- **Fewer false session endings during temporary download failures**

  Some failed downloads could be treated as a stronger "stream is offline"
  signal than they really were. rust-srec now keeps better track of which
  download engine and stream type reported the failure, so temporary network
  trouble is less likely to cut a recording into separate sessions.

- **Recording schedules now end sessions more cleanly**

  When a stream leaves its allowed recording schedule, rust-srec now closes the
  active session through the normal session flow. This keeps the dashboard,
  notifications, post-processing, and the session timeline aligned with the
  schedule decision.

- **Repeated out-of-schedule checks are quieter**

  If a streamer is already out of schedule and has no active recording,
  repeated checks no longer keep rewriting the same session state in the
  background.

- **Session timeline data is more consistent**

  Session timeline events are now handled the same way as the rest of the
  session detail data. This does not change how the timeline looks, but it
  makes the page more reliable when older or malformed event details are
  present.
