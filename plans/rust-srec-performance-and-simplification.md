# rust-srec performance and simplification: open items

Recorded: 2026-10-15; trimmed 2026-10-17. Everything else from the original investigation (F01, F02, F04–F16 except the open part of F15, S01–S08) is implemented or closed on measured evidence; the full findings, measurements and validation record are in git history for this file. F02 copy cancellation and progress now cover Linux, Windows and macOS. A transfer blocked inside the file system is surfaced (stalled operations, interrupted outputs) rather than interrupted, and a per-destination circuit breaker is deferred until those reports show a need. The two optional simplifications were implemented on 2026-10-17: `ResolvedStreamerContext` was folded into `Arc<MergedConfig>` (e3ead942), and the Mesio protocol downloaders now hold the `DownloadHandle` (7495b3c0). Line references were dropped because the code has moved since the reviewed revision.

Here, preserving behavior includes response shapes, validation and error handling, ordering, retries, cancellation ownership, filesystem safety, and Rust interfaces used elsewhere in the workspace. Larger scheduling, caching, and resource-admission changes need explicit contract checks rather than being treated as routine cleanup. Any new index needs a new migration, never an edit to an applied one.

## F03. Monitor concurrency setting controls idle connections — deferred

Deferred by user request on 2026-10-15; reopen explicitly.

[`StreamMonitor` construction](../rust-srec/src/monitor/service.rs) passes `max_concurrent_requests` (default 10) to the detector's `pool_max_idle_per_host` parameter, which [the HTTP client](../rust-srec/src/utils/http_client.rs) applies as an idle-connection limit rather than an active-check limit. Rate/backoff admission does not bound simultaneous slow extractions across streamers.

A shared extraction semaphore could bound active work; keep its wait inside the existing operation deadline and configure idle retention separately. This changes admission timing and must be tested with stalled extractions, deadlines, cancellation, and managed/anonymous checks.

## F15. Historical streamer IDs remain resident — open lead

Bounded failure diagnostics and temporary Danmu setup-lock cleanup are done. Scheduler configuration revisions, feedback identities/epochs/capacity state, dispositions, attempt identities and heartbeat timestamps still retain entries for deleted streamers. A 64-bit layout model estimates about 608 bytes per retired ID (about 60.8 MB per 100,000 IDs before allocator slack); this is a storage model, not production RSS evidence.

Revisions and feedback state are correctness fences: dropping them before older configuration jobs, producers, permits and feedback envelopes settle can admit stale work or split admission ownership. Cleanup would need a permanent-retirement point after all of those settle. Heartbeat timestamps are an optional throttle cache and could expire independently, but late progress can repopulate them. Reopen with evidence of owner churn large enough to matter.
