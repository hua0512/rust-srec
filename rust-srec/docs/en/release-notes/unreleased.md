# Release Notes

## `unreleased`

This update rebuilds the **Mesio** HLS recording engine for robustness and unifies how Mesio downloads HLS and FLV streams. Encrypted HLS streams are handled more reliably, memory use stays bounded on busy or encrypted streams, and segment de-duplication now survives playlist refreshes that rotate auth tokens such as Twitch signed URLs. A few Mesio engine settings that no longer affected recording were removed, and existing configurations are migrated automatically.

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
