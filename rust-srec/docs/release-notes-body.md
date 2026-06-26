## rust-srec (unreleased)

This update rebuilds the **Mesio** HLS recording engine for robustness and unifies how Mesio downloads HLS and FLV streams. Encrypted HLS is handled more reliably, memory use stays bounded on busy or encrypted streams, and segment de-duplication now survives playlist refreshes that rotate auth tokens such as Twitch signed URLs.

### Highlights
- **Rebuilt HLS recording engine** — the Mesio HLS engine now runs on a single control loop that owns all download state, with in-flight downloads, decryption work, and output buffers each bounded by explicit memory budgets, so a fast or encrypted stream can no longer grow memory without limit.
- **More reliable encrypted (AES-128 / fMP4) HLS** — decryption runs off the main scheduling loop and is memory-gated, fMP4 init segments are guaranteed to be written before the media that depends on them, and a terminally failed init becomes a visible gap instead of stalling the recording.
- **De-duplication survives rotating auth tokens** — segments are no longer re-downloaded when a playlist refresh rotates signatures or tokens; for known token schemes (e.g. Twitch signed URLs) the rotating parameters are stripped, and a signed URL that expires mid-download is retried transparently.
- **Explicit gaps instead of silent stalls** — when the live window slides and segments drop out before they can be fetched, the engine emits a clear gap signal so missing data is observable rather than appearing as a frozen recording.
- **Unified Mesio HLS/FLV download sessions** — HLS and FLV downloads now share one session model, so progress reporting, retry handling, and cancellation behave consistently across both protocols.

### Review before upgrading
- A database migration removes several Mesio HLS engine settings that no longer affected recording — the **Performance** tab (batch-scheduler and zero-copy toggles) plus the streaming-threshold and raw-segment-cache fields. Existing configurations are cleaned up automatically; no action is required, and the remaining timeout, retry, decryption-key, and gap-skip settings continue to work as before.

See the [unreleased release notes](https://docs.srec.rs/en/release-notes/unreleased) for the full list and the Chinese version at [/zh/release-notes/unreleased](https://docs.srec.rs/zh/release-notes/unreleased).
