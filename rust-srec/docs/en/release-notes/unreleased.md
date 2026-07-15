# Release Notes

## `unreleased`

This update rebuilds the **Mesio** HLS recording engine for robustness and unifies how Mesio downloads HLS and FLV streams. Encrypted HLS streams are handled more reliably, memory use stays bounded on busy or encrypted streams, and segment de-duplication now survives playlist refreshes that rotate auth tokens such as Twitch signed URLs. A few Mesio engine settings that no longer affected recording were removed, and existing configurations are migrated automatically.

FLV and HLS media processing also uses less CPU, memory, and disk I/O. Media analysis is reused instead of repeated, queues are bounded by bytes for large HLS segments, and FLV metadata is patched in place without shifting the completed file. Modern enhanced-RTMP keyframes and several truncated or malformed-input cases are handled more reliably as well.

Douyu extraction also gets a smaller but useful cleanup: audio-only streams can now be selected directly from the quality picker, H.265 streams are identified from Douyu's own CDN metadata, and more "room unavailable" responses are handled as offline states instead of noisy extraction failures.

The dashboard's theme system was rebuilt as well. Dark mode and custom themes now apply before the first frame on both the web and desktop apps — the desktop app no longer flashes white on launch in dark mode — and switching between light and dark is smoother and more predictable.

## Platforms

- **New platform: SOOP**

  You can add SOOP rooms (formerly AfreecaTV, `play.sooplive.co.kr` / `play.sooplive.com`) with native multi-quality HLS recording. Password-protected rooms and login-required broadcasts are supported via platform settings or cookies. When you configure a SOOP account, the app validates and renews session cookies automatically so later checks reuse the login. Live chat plus gifts can be recorded when danmaku is enabled.

- **New platform: Bigo Live**

  You can add Bigo Live rooms (`bigo.tv`) as streamers with native HLS recording. Password-protected rooms are supported via platform settings or a `?pwd=` URL parameter, and live chat plus gifts can be recorded when danmaku is enabled. Website-style integrity token minting is on by default for better API parity. The public stream is a single mid-quality portrait feed (typically around 480p–540p); higher browser-only qualities are not available on this path.

## Desktop app

- **No more white flash when launching in dark mode**

  The main window previously appeared before the saved theme was applied, so dark-mode users saw a white frame on every launch. The theme is now applied before the window is shown, and the loading splash screen follows your chosen theme instead of only the operating system setting — so a light-OS user who prefers a dark app gets a dark splash and a dark first frame.

- **Windows GPU checks no longer flash console windows**

  On Windows systems with NVIDIA GPUs, the desktop app could briefly flash black console windows at startup and every time the background GPU health check ran. GPU checks now run without opening console windows, and startup no longer performs an extra back-to-back check.

## Theme and appearance

- **Custom themes apply at first paint**

  A saved theme preset, imported theme, or color override is now restored before the page first renders, on both web and desktop — no more brief flash of the default palette on load. After updating the app, your saved theme keeps applying immediately unless the app's theme data itself changed in the update.

- **Smoother, more reliable dark/light switching**

  The circular reveal animation no longer occasionally flips the whole screen to the new theme before the animation starts. The header toggle now switches based on the appearance you currently see, so the first click always has an effect when the mode is set to **System**. Selecting a mode that looks identical to the current one (for example switching from **Dark** to **System** while the OS is dark) saves the preference without playing a pointless animation.

- **Theme settings stay in sync across browser tabs**

  Changing the preset, colors, or radius in one tab now updates every other open tab, matching how the light/dark mode already behaved.

- **Delete confirmation buttons show the right text color**

  A missing style definition left the text on some destructive confirmation buttons (template, notification channel, and workflow deletion) rendering in the wrong color. They now use the theme's destructive foreground color.

## HLS recording engine

- **Rebuilt HLS engine for more predictable recording**

  The Mesio HLS engine was rebuilt around a single control loop that owns all download state — which segments to fetch, retry deadlines, and completion tracking — instead of spreading it across several cooperating tasks. Recording behavior is now deterministic under load: in-flight downloads, decryption work, and output buffers are each bounded by explicit memory budgets, so a fast or encrypted stream can no longer grow memory without limit.

- **More reliable encrypted (AES-128 / fMP4) HLS**

  Decryption now runs off the main scheduling loop and is memory-gated, so a burst of encrypted segments stays responsive instead of piling up. For fragmented-MP4 streams, the engine guarantees the init segment is written before the media that depends on it, avoiding codec-mismatch corruption, and a terminally failed init is surfaced as a visible gap instead of stalling the recording.

- **Segment de-duplication survives rotating auth tokens**

  Segments are no longer re-downloaded when a playlist refresh rotates query parameters such as signatures or tokens. For sources with known token schemes (for example Twitch signed URLs), the engine can strip the rotating parameters so the same segment is recognized across refreshes, and a signed URL that expires mid-download is retried transparently against a newer one.

- **Skipped segments and gaps are explicit**

  When the live window slides and segments drop out before they can be fetched, the engine emits a clear gap signal instead of silently stalling, so missing data is observable rather than appearing as a frozen recording.

## FLV and HLS media processing

- **Lower processing overhead with bounded memory use**

  FLV codec and keyframe classification and HLS transport-stream analysis are now computed once and reused throughout the repair pipeline. Resolution detection reads only a bounded amount of stream data, processing work is batched, and HLS queues are limited by byte budgets rather than segment counts. Byte accounting remains attached while a batch is being processed, so batching cannot temporarily bypass that limit. If fragmented-MP4 media exhausts its pre-init safety buffer before a delayed init segment arrives, the repair pipeline rotates the output before writing that init. This reduces repeated parsing and prevents a handful of unusually large segments from causing disproportionate memory growth or invalid file ordering.

- **FLV metadata is updated in place**

  The writer now reserves baseline metadata space when recording starts, including for audio-only streams and recordings without a keyframe index. When keyframe indexing is enabled, the larger index reservation is patched before the writer closes. The repair step no longer shifts or rewrites the rest of a completed FLV file, reducing disk I/O and avoiding a costly end-of-recording pause on large files. The legacy low-latency CLI option remains accepted for compatibility, but metadata updates always use the in-place path.

- **More resilient FLV repair**

  Enhanced-RTMP video keyframes are recognized correctly, complete tags at end-of-file are no longer skipped, empty media payloads are handled safely, and GOP buffering has a hard limit. These changes improve splitting and metadata generation for newer codecs while keeping damaged or unusual streams from growing repair buffers without bound.

## Mesio downloader

- **Unified HLS and FLV download sessions**

  Mesio's HLS and FLV downloaders now share a single session model, so progress reporting, retry handling, and cancellation behave consistently across both protocols.

- **Simplified Mesio engine settings**

  Several Mesio HLS settings that no longer affected recording were removed from the engine settings form, including the **Performance** tab (the batch-scheduler and zero-copy toggles) along with the streaming-threshold and raw-segment-cache fields. Stored configurations are cleaned up automatically by a database migration — no action is required, and the remaining timeout, retry, decryption-key, and gap-skip settings continue to work as before.

## Douyu streams

- **Audio-only is part of the quality picker**

  Douyu's quality setting now includes an **Audio only** option alongside the usual quality presets. You can still type a custom Douyu rate when needed, but common choices such as original quality, HD, low quality, and AAC audio are available from the same control.

- **H.265 detection follows Douyu CDN metadata**

  Douyu stream entries now use the `isH265` value returned by Douyu's CDN list to mark HEVC streams. When a selected CDN supports H.265, the extractor uses Douyu's dedicated `player_1` URL and falls back to the standard stream URL if it is unavailable. There is no separate frontend switch to manage, and audio-only requests continue to use the AAC stream response.

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

- **Disabling a streamer can no longer be undone by an in-flight status check**

  Disabling a streamer while one of its status checks was still in flight could let that check re-mark the streamer as live, create a recording session, and start a download after the disable. Session creation now re-checks the streamer's state at the database serialization point, so a disable that has been saved always wins — the late check is discarded instead of overriding user intent.
