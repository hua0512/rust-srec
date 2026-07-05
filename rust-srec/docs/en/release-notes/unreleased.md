# Release Notes

## `unreleased`

This update rebuilds the **Mesio** HLS recording engine for robustness and unifies how Mesio downloads HLS and FLV streams. Encrypted HLS streams are handled more reliably, memory use stays bounded on busy or encrypted streams, and segment de-duplication now survives playlist refreshes that rotate auth tokens such as Twitch signed URLs. A few Mesio engine settings that no longer affected recording were removed, and existing configurations are migrated automatically.

Douyu extraction also gets a smaller but useful cleanup: audio-only streams can now be selected directly from the quality picker, H.265 streams are identified from Douyu's own CDN metadata, and more "room unavailable" responses are handled as offline states instead of noisy extraction failures.

## HLS recording engine

- **Rebuilt HLS engine for more predictable recording**

  The Mesio HLS engine was rebuilt around a single control loop that owns all download state — which segments to fetch, retry deadlines, and completion tracking — instead of spreading it across several cooperating tasks. Recording behavior is now deterministic under load: in-flight downloads, decryption work, and output buffers are each bounded by explicit memory budgets, so a fast or encrypted stream can no longer grow memory without limit.

- **More reliable encrypted (AES-128 / fMP4) HLS**

  Decryption now runs off the main scheduling loop and is memory-gated, so a burst of encrypted segments stays responsive instead of piling up. For fragmented-MP4 streams, the engine guarantees the init segment is written before the media that depends on it, avoiding codec-mismatch corruption, and a terminally failed init is surfaced as a visible gap instead of stalling the recording.

- **Segment de-duplication survives rotating auth tokens**

  Segments are no longer re-downloaded when a playlist refresh rotates query parameters such as signatures or tokens. For sources with known token schemes (for example Twitch signed URLs), the engine can strip the rotating parameters so the same segment is recognized across refreshes, and a signed URL that expires mid-download is retried transparently against a newer one.

- **Skipped segments and gaps are explicit**

  When the live window slides and segments drop out before they can be fetched, the engine emits a clear gap signal instead of silently stalling, so missing data is observable rather than appearing as a frozen recording.

## Mesio downloader

- **Unified HLS and FLV download sessions**

  Mesio's HLS and FLV downloaders now share a single session model, so progress reporting, retry handling, and cancellation behave consistently across both protocols.

- **Simplified Mesio engine settings**

  Several Mesio HLS settings that no longer affected recording were removed from the engine settings form, including the **Performance** tab (the batch-scheduler and zero-copy toggles) along with the streaming-threshold and raw-segment-cache fields. Stored configurations are cleaned up automatically by a database migration — no action is required, and the remaining timeout, retry, decryption-key, and gap-skip settings continue to work as before.

## Douyu streams

- **Audio-only is part of the quality picker**

  Douyu's quality setting now includes an **Audio only** option alongside the usual quality presets. You can still type a custom Douyu rate when needed, but common choices such as original quality, HD, low quality, and AAC audio are available from the same control.

- **H.265 detection follows Douyu CDN metadata**

  Douyu stream entries now use the `isH265` value returned by Douyu's CDN list to mark HEVC streams. There is no separate frontend switch to manage, so recordings follow the format Douyu reports for the selected CDN and rate.

- **More unavailable-room responses are treated cleanly**

  Douyu error codes `-3`, `-4`, and `-5` are now handled as unavailable or offline stream states. This reduces false hard failures when a room closes, the streamer goes offline during extraction, or Douyu returns a temporary unavailable response.

## Pipeline uploads

- **Session-start date anchors for upload destinations**

  Rclone pipeline steps now include a `time_anchor` setting. Keep the default `job_created` behavior, or choose `session_start` so date placeholders such as `%Y/%m/%d` use the live session's start time and a stream crossing midnight stays in one dated remote folder. Copy/move steps can also opt into `job_created` or `session_start` anchoring while preserving their existing execution-time behavior by default.

- **Upload destination dates use server local time**

  Time placeholders in upload destination paths now render in the server's local time zone when an explicit reference timestamp is used. They previously rendered in UTC, which could put uploads in a different dated directory than local recording filenames on non-UTC deployments. The time zone offset is taken from the moment being rendered, so recordings around daylight-saving transitions keep the date their filenames carry, and retried uploads land in the same dated folder as the original run.

## Pipeline step forms

- **Placeholder names show up in help texts again**

  Some pipeline step forms rendered placeholder help text with the names missing — for example the danmaku conversion step showed "Use  and  placeholders." instead of naming `{input}` and `{output}`. The rclone and copy/move destination fields now also document the supported placeholders directly in the form.

- **Clearer error for invalid copy/move step configuration**

  A copy/move step whose saved configuration fails to parse (for example a mistyped option value) now reports the actual parse error instead of a misleading "no destination directory specified" message.

## Recording reliability

- **Recording recovers on its own after repeated download start failures**

  When a stream's CDN briefly rejected downloads (for example Douyu edge nodes returning HTTP 404 while the room was still live), three consecutive failures put the streamer into a short error backoff. A timing race between that backoff and the session-resume path could leave the recording session marked active with no download running — the room kept showing as live with an error, but recording never restarted until monitoring was toggled manually. The download-start gate now reports this refusal the same way other rejected downloads are reported: the session closes cleanly and a status check is scheduled for the moment the backoff expires, so recording resumes automatically once the stream is reachable again.

- **Live watchdog restarts downloads that disappeared silently**

  As a safety net for any path that drops a download start without feedback, the periodic live watchdog now recognizes "reported live, but no download activity for five minutes" and re-triggers the download start instead of discarding the check result.
