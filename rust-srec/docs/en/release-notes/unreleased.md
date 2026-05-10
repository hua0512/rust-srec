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

- **Removed an inactive session timing setting**

  The old session gap time setting has been removed because it no longer
  affected recordings. Stream ending is controlled by Offline Check Delay and
  Offline Detection Count. Backups that still contain the old value continue to
  import successfully, and the value is ignored.

- **Queue freshness settings now save correctly**

  Changes to the queue freshness threshold are now stored reliably, so rust-srec
  keeps your chosen re-check timing after settings updates.

## System health

- **GPU health is now tracked on the System Health page**

  If your container loses GPU access (a known issue with the NVIDIA Container
  Toolkit on cgroup v2 hosts), you'll get a notification right away instead of
  finding out from the next failed remux job. The probe interval is
  configurable from the global settings page.

- **`/api/health` is faster and lighter on resources**

  The dashboard's health endpoint now reads a cached snapshot instead of
  re-running every check on each poll. Health components refresh in the
  background on per-component cadences (cheap atomic checks every 5 s, disk
  capacity every 30 s), so opening the System Health page is instant and
  background CPU stays low even on busy systems.
