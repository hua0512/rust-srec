# rust-srec performance findings and behavior-preserving simplifications

Recorded: 2026-10-15. Reviewed revision: `c314be4a7180fa03a3898f319ec0bb9985f6ea75`.

Status: the first performance batch was merged in [PR #1122](https://github.com/hua0512/rust-srec/pull/1122). The findings below retain the investigation record at the reviewed revision. Scope is the recorder backend, with frontend callers inspected to establish polling workloads. No live database or recording data was opened or modified. Measurements below are isolated probes, not application-wide benchmarks, and do not establish production speedups. Source line anchors describe the reviewed revision; implementations can move those lines.

Deferred by user request on 2026-10-15: **F03**, **F09**, and **F10**. Their evidence and recommendations remain recorded, but these items are excluded from the current implementation scope until explicitly reopened.

## Implementation progress

| Finding | Implemented change | Status |
| --- | --- | --- |
| F01 | Cancellation returns a nonretryable compression error; bounded retry-loop regression added. | Implemented and validated. |
| F02 | Native copies retain staged-output and worker-completion ownership until the blocking operation exits. Cancellation observed before publication rejects the staged copy; final rename retains its existing semantics. Windows cleanup handles read-only private temps without changing hard-linked originals. | Partial: native mid-copy interruption remains open. Native copy acceleration and platform metadata are preserved. |
| F04 | Staging uses indexed path/identity snapshots without retaining input handles; commit prepares fresh validation. | Implemented and validated. External path replacement can defer rejection from staging to commit. |
| F05 | Unchanged feedback targets use a registry membership revision check; feedback retirement invalidates it. | Hot-path fix implemented and validated. Actual membership changes still copy the target map. |
| F06 | Existing log worker batches up to 32 records / 64 KiB, sharing coordination and inventory while preserving per-record rotation/truncation. | Implemented and validated. Normal shutdown flushes; abrupt termination can lose the bounded worker buffer. |
| F08 | DAG list summaries omit unused manifests and retain existing name parsing. The unused full-record list (`list_dags_by_display_status`) was removed from `DagRepository` and `DagScheduler`; no workspace consumer used it. | Implemented and validated. |
| F11 | Indexed protected-job selection is materialized for candidate and transactional final checks; a new partial index covers nonterminal IDs. | Implemented and validated. New migration: `20261015120000_index_output_retention_protected_jobs.sql`. |

F07, F12, F13, F14, F15, and F16 remain investigated and unimplemented. S01–S03 are implemented and validated in this follow-up: the media driver uses `OutputBatch` directly, unused private scheduler state is removed, and remux updates its existing result. S05 was completed with F08; S04 and S06–S08 remain recommendations.

Performance batch validation on Windows with Rust 1.98.1 and default features:

- `cargo fmt --all -- --check`: passed.
- `cargo clippy --locked -p rust-srec --all-targets -- -D warnings`: passed.
- Nextest selected compression/copy, output/path safety, processor publication, scheduler, DAG list, retention, logging/API, worker-completion, and `database_index_budget` coverage: **364 tests passed**, 2,239 outside the selection skipped.
- Migration upgrade/replay, integrity checks, protected-job membership, query plans, malformed/duplicate DAG names, final retention rechecks, and mixed batched/unbatched log processes are covered by that run.
- Deferred F03/F10 source files and the F09 DAG-statistics implementation were compared against the original revision and are unchanged. Report links and whitespace were checked.

The test selection was:

```powershell
cargo nextest run --locked -p rust-srec --lib --test database_index_budget --no-fail-fast --test-threads 8 -E 'test(pipeline::processors::compression::) | test(pipeline::processors::copy_move::) | test(pipeline::processors::paths::) | test(pipeline::processors::outputs::) | test(pipeline::processors::output_tests::) | test(scheduler::) | test(output_retention) | test(dag_summaries) | test(api::routes::pipeline::dag::) | test(logging::) | test(retained_log_sink) | test(timeout_log_drain) | binary(database_index_budget)'
```

An isolated comparison of the original pairwise validation helpers against the implemented index measured 128-file validation at 3.974 s versus 23.85 ms, excluding media work. A passing resource regression verifies that 32 buffered log records require one directory inventory instead of 32 and produce identical content. These are targeted measurements, not overall recorder throughput claims.

Unix-only symlink cases do not execute on Windows; platform coverage remains with CI. The full package/workspace test suites were not run. The first retention run exposed SQLx's handling of mixed numbered/anonymous placeholders; all eligibility suffixes now use explicit numbering and the existing/new retention tests pass.

For S01–S03, formatting and affected-package Clippy with all targets passed on Windows with the same toolchain/default features. The focused suite below passed **327 tests**, including the existing FFmpeg thumbnail fallback test, and skipped 2,274 outside the selection. Coverage includes processor publication, failure/cancellation cleanup, remux skip/deletion behavior, scheduler configuration scopes, and streamer retirement. The existing remux contract now also checks metadata and removal-log order; no new tests or helpers were added. The platform and workspace limitations above still apply.

```powershell
cargo nextest run --locked -p rust-srec --lib --run-ignored all --no-fail-fast --test-threads 8 --status-level fail --final-status-level fail -E 'test(scheduler::) | test(streamer_retirement_tests::) | test(pipeline::processors::remux::) | test(pipeline::processors::audio_extract::) | test(pipeline::processors::metadata::) | test(pipeline::processors::thumbnail::) | test(pipeline::processors::outputs::) | test(pipeline::processors::paths::) | test(pipeline::processors::output_tests::)'
```

## Recommended starting points

**F01 (ZIP cancellation)** and **S05 (DAG summary projection, implemented with F08)** are complete. **S01 (publication abstraction)**, **S02 (unused scheduler state)**, and **S03 (remux result handling)** form the current simplification batch. Evaluate S04 and S06–S08 individually, keeping changes only when the removed duplication outweighs any added interface. Larger scheduling, caching, and resource-admission changes require explicit contract checks rather than being treated as routine cleanup.

Here, preserving behavior includes response shapes, validation and error handling, ordering, retries, cancellation ownership, filesystem safety, and public Rust interfaces. A candidate marked behavior-preserving still needs implementation and regression verification.

## Performance and resource findings

### F01. Cancelled ZIP compression can spin indefinitely — correctness fix

[CancelProgressReader::read](../rust-srec/src/pipeline/processors/compression.rs#L155) returns `ErrorKind::Interrupted` once cancelled. The [ZIP copy loop](../rust-srec/src/pipeline/processors/compression.rs#L372) uses `std::io::copy`, which retries that error. The cancellation condition remains true, so a mid-file cancellation can leave the blocking worker spinning, retaining resources and delaying runtime shutdown. Timeouts, explicit cancellation, shutdown, and dropping the processor future can reach this path.

Return a nonretryable cancellation error or use an explicitly cancellation-aware copy loop. Verify that the blocking worker exits after cancellation during copying. This restores intended cancellation behavior; it is not a pure refactor. ZIP was reproduced; TAR uses the same reader, but its retry behavior was not verified.

### F02. Cancelled file copies outlive their worker slot — cancellation/resource fix

[CopyMoveProcessor::copy_via_tmp](../rust-srec/src/pipeline/processors/copy_move.rs#L148) awaits `tokio::fs::copy`. Locked Tokio 1.53.2 implements this as a blocking whole-file copy. Dropping its future does not stop an already running copy, while the pipeline can release its I/O admission and start another job. Repeated cancellation/retry can overlap large copies or cross-filesystem moves beyond the configured worker count. Temporary cleanup can also finish before the copy closes its destination.

Consider cooperative copying that retains temporary-file ownership and the admission permit until the operation actually exits. Preserve overwrite, verification, disk-full reporting, move-source deletion, and publication behavior. No large-file cancellation benchmark was run.

### F03. Monitor concurrency setting controls idle connections — deferred

Status: deferred by user request. Proposed work is an admission change.

[StreamMonitor construction](../rust-srec/src/monitor/service.rs#L371) passes `max_concurrent_requests` (default 10) to the detector's `pool_max_idle_per_host` parameter. [Client construction](../rust-srec/src/utils/http_client.rs#L112) applies it as an idle-connection limit, not an active-check limit. Rate/backoff admission does not bound simultaneous slow extractions across streamers.

A shared extraction semaphore could bound active work; keep its wait inside the existing operation deadline and configure idle retention separately. This changes admission timing and must be tested with stalled extractions, deadlines, cancellation, and managed/anonymous checks.

### F04. Batch output validation performs quadratic filesystem work — optimization candidate

[OutputBatch validation](../rust-srec/src/pipeline/processors/outputs.rs#L145) compares every destination with all sources and preceding destinations, both during staging and before commit. [Each comparison](../rust-srec/src/pipeline/processors/paths.rs#L102) performs filesystem identity and path-resolution work. With N inputs and N outputs, this is `3N² - N` comparisons, plus repeated path cloning.

This matters for session-complete/manual batches; per-segment batches are small. Prepare source identities/resolved keys and destination indexes once per validation phase. Preserve hardlink/symlink protection, I/O error handling, and fresh commit-time checks; do not simply remove validation or reuse stale stage-time identities at commit.

### F05. Scheduler scans all targets after every event — optimization candidate

[The scheduler loop](../rust-srec/src/scheduler/service.rs#L710) synchronizes feedback targets after every iteration. [update_targets](../rust-srec/src/scheduler/feedback.rs#L335) compares every actor generation under the shared feedback mutex even when nothing changed. Download progress can be broadcast every 250 ms, giving roughly `4 × progressing downloads × actors` comparisons/second at that cadence. Actor startup also repeatedly copies the growing target map.

Publish membership/generation changes through one revision-aware interface. Preserve feedback retirement and replacement ordering; feedback can invalidate targets independently of registry membership, so a registry revision alone is insufficient. Coalescing statistics publication is a separate timing change.

### F06. File logging repeats filesystem coordination per record — batching candidate

[LogStore::append](../rust-srec/src/logging/store.rs#L311) acquires the cross-process lock, inventories managed files, checks retention, and opens/closes the destination for each write. The nonblocking appender moves this work off callers but still invokes the writer for each queued record.

Bounded batching could amortize this work. Preserve record truncation boundaries, byte/count limits, dates, cross-process coordination, archive/retention safety, and the nonblocking emergency path. Grouping bytes in a probe is not a drop-in implementation: batching also changes visibility latency and can change rotation semantics unless designed carefully.

### F07. Configuration is fully resolved twice — optimization with freshness constraints

[Startup metadata refresh](../rust-srec/src/services/container.rs#L332) populates cached resolved configurations for hydrated streamers, then [scheduler configuration](../rust-srec/src/scheduler/service.rs#L132) deliberately reads fresh layers for active monitored streamers to consume two timing fields. Global/platform/template changes repeat both paths for affected monitored streamers. Full resolution includes configuration, credential-selection, and route work.

A shared revisioned result could remove duplicate work. Do not replace the fresh call with a cache lookup: it protects a publication race. A timing-only resolver must also preserve existing validation/failure behavior; currently a failed full resolution can preserve old settings or defer an actor restart.

### F08. DAG lists copy unused large JSON into Rust — projection candidate

[list_dags_by_display_status](../rust-srec/src/database/repositories/dag.rs#L744) selects `dag.*`, including session-wide `input_manifest` data that [the response mapping](../rust-srec/src/api/routes/pipeline/dag.rs#L359) discards. A long-session page can copy megabytes unnecessarily. This is transfer/allocation between SQLite and Rust; HTTP responses already omit these manifests.

Use a summary projection. The safest first version retains `dag_definition` and the [existing partial Serde name parser](../rust-srec/src/database/models/dag.rs#L96), while omitting manifests and other unused columns. SQL name extraction is not automatically equivalent on malformed definitions, missing/non-string names, or duplicate `name` keys. See S05.

### F09. Dashboard statistics scan retained DAG history — deferred

Status: deferred by user request. Proposed work is a query/cache optimization.

[get_dag_display_counts](../rust-srec/src/database/repositories/dag.rs#L797) groups a display-status expression over all DAG rows and separately averages completed durations. [The frontend statistics call](../rust-srec/frontend/src/server/functions/pipeline.ts#L203) uses `/pipeline/dags/stats`; dashboard/jobs views poll every 10 seconds. These are DAG queries, not the separate job-statistics queries.

Consider evaluating special display-status rules only for active rows, using covering aggregates for terminal rows, or sharing refreshes across clients. Extra indexes have storage/write costs. A cache introduces staleness, so it is not a strict behavior-preserving refactor. Impact depends on retained history and client count; default history retention is 30 days.

### F10. Streamer pages enrich every configured owner — deferred

Status: deferred by user request. Proposed work narrows query scope.

[list_streamers](../rust-srec/src/api/routes/streamers.rs#L552) loads every credential selection and all streamer routes before applying pagination. Fetching enrichment for page IDs would reduce reads and allocations on large installations.

Preserve filtering, total counts, sorting, and response fields. Narrowing enrichment can change failure visibility: malformed off-page configuration can currently fail the request. Empty pages currently still execute enrichment. Resolve that contract before claiming strict equivalence.

### F11. Output retention repeatedly scans job history — conditional query optimization

[The eligibility predicate](../rust-srec/src/database/maintenance/outputs.rs#L18) correlates each output with job session/raw-input/JSON-input protection. Query plans show `SCAN j` under a correlated subquery. The predicate is repeated during per-file checks inside the serialized writer transaction. Limiting candidate count does not bound examined job rows.

Output retention defaults to disabled. When enabled, first identify active/retry/recent job IDs using indexed predicates, then evaluate protection against that set. Preserve unscoped jobs, raw/JSON inputs, malformed JSON handling, and the transactional final recheck. Output retention runs before history pruning; old retained jobs are realistic when output retention is shorter than job retention or history is unlimited. Do not reorder cleanup to sidestep the protection policy.

### F12. Notification dispatch has no global work bound — overload-policy change

[dispatch_notification](../rust-srec/src/notification/service.rs#L548) spawns before event-log persistence and queue admission. [Delivery concurrency](../rust-srec/src/notification/service/delivery.rs#L195) is four per notification rather than a global limit. Slow database writes or delivery endpoints can accumulate tasks/connections during bursts. Eviction cancels scheduled retries but does not interrupt an already awaited channel send.

Bound ingress and total delivery work only with an explicit overload policy. Preserve event logging, web push independence, subscription/channel snapshots, retries, and shutdown ownership. Do not silently add drops or change ordering in a cleanup refactor. The duplicated construction itself can be simplified independently (S04).

### F13. Rate-limit waiters repeatedly wake together — admission-policy change

[RateLimiterManager::acquire](../rust-srec/src/monitor/rate_limiter.rs#L220) makes callers wait until the same next-token deadline, contend on a cross-platform mutex, and retry. [Initial actor checks](../rust-srec/src/scheduler/actor/messages.rs#L646) are immediate; recurring jitter does not spread startup. This matters for large restarts and forced-check bursts, not small idle installations.

FIFO admission per platform could avoid repeated wakeups, but changes ordering/fairness. Preserve rate/burst limits, cancellation without consuming tokens, backoff, and deadlines. The operation-count argument is analytical; no scheduler load benchmark was run.

### F14. Status polling uses the single writer connection for a read — contention lead

[check_streamer_for](../rust-srec/src/monitor/service.rs#L673) reads active-session binding through `write_pool` before in-flight deduplication, including offline anonymous checks. The writer pool has one connection. This is a verified extra acquisition; its practical contention cost is unmeasured.

Measure pool waits during poll bursts and lifecycle writes. Moving the query to readers or a lifecycle-owned snapshot requires evidence that binding freshness and operation ordering remain correct; changing pools alone is not yet a proven safe optimization.

### F15. Some diagnostics and historical IDs remain resident — lower-priority retention leads

[Danmu collection failures](../rust-srec/src/danmu/service.rs#L1002) accumulate strings until shutdown. Per-streamer start-serialization entries also have no removal path. Scheduler revision/retirement and some heartbeat/feedback maps retain historical streamer IDs after deletion. These grow with errors or owner churn, rather than active recordings alone; large practical impact was not measured.

Consider bounded diagnostic samples plus total counts, and permanent-retirement cleanup only after old jobs, feedback envelopes, and permits settle. Capping diagnostics changes shutdown-report contents; dropping generation/retirement state too early can reintroduce stale work. Existing session cancellation handles already clean up their registry on drop and are not a missing-cleanup finding.

### F16. Full Processing adds metadata through another full media pass — opt-in workflow optimization

The shipped [Full Processing preset](../rust-srec/migrations/20250920193717_initial_schema.sql#L1183) remuxes and then invokes a separate metadata processor. [Metadata embedding](../rust-srec/src/pipeline/processors/metadata.rs#L133) copies the complete input into a new file; [remux already accepts tags](../rust-srec/src/pipeline/processors/remux.rs#L456).

An optimized preset could save one full recording read/write and one recording-sized intermediate. This changes named steps, intermediate files, and per-step retries. Offer an explicitly optimized workflow; do not silently rewrite custom workflows or edit shipped migrations. Savings are structural; no media benchmark was run.

## Measurement record

Scratch probes were not committed as a benchmark suite. Reproduce measurements against the implementation under consideration before accepting a speedup claim.

| Probe | Setup and scope | Observed result |
| --- | --- | --- |
| ZIP interrupted reads | Bounded Rust executable using the repository-built ZIP dependency, `ZipWriter<BufWriter<File>>`, ZIP64 and Deflate level 6; one successful read, then 1,000,000 interrupted reads, then a deliberate nonretryable escape | More than 1,000,000 read attempts in about 70 ms; stopped only at the escape. Production cancellation has no such escape. |
| Log appends | Actual `logging/store.rs` included in a standalone optimized Rust 1.98.1 probe; local Windows temporary directory, 16 managed files, 207-byte lines, five samples of 300 writes per configuration; no rotation during timing | Median 180.9 microseconds per line individually; 6.60 microseconds per line with 32 lines grouped into each append. Does not measure full tracing overhead or preserve all record semantics by itself. |
| Output validation | Current synchronous validation/path helpers extracted with visibility/error scaffolding adapted; repository `same_file` dependency; one-byte existing sources and missing output leaves; stage plus commit checks | 32 files: 3,040 comparisons / 0.244 s. 64: 12,224 / 0.980 s. 128: 49,024 / 7.83 s. Windows filesystem/antivirus variability applies; media work excluded. |
| Retention candidate query | Python SQLite 3.50.4, fresh in-memory database with every migration applied; 500 old outputs and 20,000 old unrelated completed jobs with JSON-array inputs; literal eligibility SQL | Candidate selection: 0.917 s current vs 0.0010 s experimental indexed protection. Five hundred individual final eligibility queries: 0.940 s vs 0.0047 s. Same candidates; transaction setup, deletion, filesystem, SQLx, and Rust overhead excluded. |
| DAG list projection | Same SQLite setup; 24 completed DAGs, each with an actual-shaped 1,000-segment manifest of 217,986 bytes; warm median of 15 queries | Full rows returned 5,234,448 text bytes in 1.629 ms; an exploratory SQL-name summary returned 744 bytes in 0.027 ms. The conservative parser-preserving projection removes 5,231,664 manifest bytes but was not separately timed. |
| DAG aggregates | Same SQLite setup; 50,000 DAGs, 2,126-byte definitions, 49,960 completed and 40 active; warm median of nine queries | Counts: 19.091 ms current vs 3.155 ms split terminal/active aggregate. Duration average: 16.921 ms current vs 1.843 ms with a covering `(status, completed_at, created_at)` index. Same results on this fixture; index maintenance cost excluded. |

The retention prototype materialized a union of IDs selected by active status, non-null retry time, or recent update, then kept the original protection predicate with an additional ID restriction. Its `PENDING`/`PROCESSING` branch matches today's five-value status CHECK constraint. A production implementation must retain protection for future nonterminal statuses, for example by retaining the original negative terminal classification with a suitable index.

## Simplifications that can preserve current behavior

### S01. Remove the single-implementation publication abstraction

[media_driver.rs](../rust-srec/src/pipeline/processors/media_driver.rs#L10) defines `Publication`, an associated publication type on `MediaItem`, and `StagedPublication(OutputBatch)`. There is exactly one production publication implementation, and its rollback method is a no-op because `OutputBatch` already owns cleanup.

Pass `OutputBatch` directly through the runner and item processors; propagate item failures normally and let its guards perform cleanup. Keep `MediaItem`, which has four real adapters. This removes a hypothetical seam and leaves publication ownership in one existing module rather than introducing another layer.

Preserve sequential input order, unary/batch aggregation, early skips, commit only after all items succeed, rollback, overwrite/no-clobber rules, and cancellation-independent commit. Validate using existing [processor driver contracts](../rust-srec/src/pipeline/processors/output_tests/driver_contracts.rs), output publication/rollback tests, and processor tests. This is entirely private to the processors.

### S02. Delete scheduler state that has no effective reader

The scheduler's private [PlatformMapping and duplicate platform_handles](../rust-srec/src/scheduler/service.rs#L205) are maintained independently of the registry. Its mapping is only written by production scheduler paths; current configuration routing uses `StreamerManager`. The duplicate handle map's lookup feeds an expression that always returns `None` at [platform sender construction](../rust-srec/src/scheduler/service.rs#L826).

Remove those private fields and their maintenance, and pass the existing `None` explicitly. Keep exported `PlatformMapping`, `ConfigRouter`, and batch interfaces for Rust source compatibility. Preserve current disabled batch detection, actor ownership/retirement, and configuration scopes. Run configuration-scope, reliability, and retirement tests.

### S03. Keep the remux result instead of dismantling and rebuilding it

[Remux processing](../rust-srec/src/pipeline/processors/remux.rs#L754) destructures the result from `run_media` and later reconstructs `ProcessorOutput` to add deletion logs and change metadata. Keep a mutable result and update only those fields, as other processors already do.

Preserve the single-skipped-input early return, source-deletion and alias checks, removed-input count, logs, metadata, and every aggregate field. This is a small local simplification with no claimed throughput gain. Existing processor driver and remux tests should compare the returned results.

### S04. Construct pending notifications once, leaving entrypoint policy explicit

[notify_channel_instances](../rust-srec/src/notification/service.rs#L344) and [notify](../rust-srec/src/notification/service.rs#L434) repeat channel-state and `PendingNotification` initialization. A private constructor can own default delivery states, locks, retry generation/token, and target snapshot setup. Separately, the identical configuration-to-channel match in [initialization and dynamic registration](../rust-srec/src/notification/service/registry.rs#L53) can share a private factory.

Keep broadcast, ID allocation, target selection, event-log persistence, and web push in their current caller-specific order. Named-channel sends bypass subscription/event-log policy; unknown targets remain errors. Do not merge these paths into a mode/boolean-driven dispatcher. Preserve channel keys, enabled filtering, reload snapshots, eviction, and retry cancellation; use existing notification delivery/reload/listener tests.

### S05. Give DAG lists a summary model without changing name parsing

Introduce a focused list summary that omits manifests and other unused columns, retaining `dag_definition` long enough to reuse the current name parser. Keep full execution retrieval for execution/recovery consumers. Reuse existing streamer-name lookup and display-status logic rather than reimplementing them in the route.

Preserve filter intersection, ordering, pagination, count behavior, progress calculation, missing streamer handling, and `"Unknown"` fallback. Differential HTTP fixtures should cover all statuses, empty/large manifests, malformed definitions, missing/non-string names, and duplicate `name` keys. If changing exported repository signatures, retain compatibility or explicitly account for consumers; a private SQL projection alone does not require removing public full-record interfaces.

### S06. Build each query predicate and its bind together

[Job predicate construction](../rust-srec/src/database/repositories/job.rs#L20) and [its binding macro](../rust-srec/src/database/repositories/job.rs#L72) must stay in matching order. [DAG filtering](../rust-srec/src/database/repositories/dag.rs#L512) repeats the same split. A private function in each repository can append predicates and values together, then serve count and data queries.

This removes the caller's obligation to keep two implementations synchronized. Do not introduce a generic query framework. Preserve `status AND statuses` intersection, empty vectors, inclusive millisecond dates, literal `%`/`_`/backslash searches, DAG blank-search trimming, and current ordering. Compare count/list/page results on combined-filter fixtures and retain existing literal-search tests.

### S07. Share job-list request conversion and response assembly

[/jobs and /jobs/page](../rust-srec/src/api/routes/pipeline/jobs.rs#L66) duplicate filter conversion and enriched response assembly. Extract local helpers while leaving their response envelopes and count policies explicit. Reuse existing `job_to_response`, `fetch_streamer_names`, and [batch_lookup::collect](../rust-srec/src/api/batch_lookup.rs).

Preserve the 100-row cap, sequential bounded related-data queries, per-owner fallback after failed batches, missing-owner responses, and the page endpoint's avoidance of total-count queries. This is a maintenance improvement, with no performance gain claimed.

### S08. Put command diagnostics formatting on the existing result type

[CommandOutput](../rust-srec/src/pipeline/processors/utils.rs) already owns log/error inspection. Audio extraction, metadata, and thumbnail processors repeat non-Info log filtering and newline joining. A method on the existing type can own that representation detail.

Keep the exact filtering, order, and joined text. Each processor retains its own failure/skip/retry classification and fallback behavior. Do not introduce a common FFmpeg processor trait or unify no-audio/no-frame policies merely to remove similar-looking code. Use existing driver skip/failure contracts and thumbnail fallback tests.

### Conditional simplifications

- **Registry target synchronization (F05):** concentrate it behind one interface, but track both registry and feedback-side invalidations. Preserve retire/remove/replacement ordering and superseded-generation completion handling.
- **Configuration resolution (F07):** extracting timing precedence is a refactor; skipping full validation or reading cached metadata is not. Preserve failed-resolution behavior before claiming equivalent results.
- **Mesio protocol constructors:** they unpack an existing `Arc<DownloadHandle>` into repeated configuration/event/token/manual-split arguments and DASH rebuilds the HLS inputs. Passing the existing handle would reduce wiring, but HLS/FLV/DASH downloader constructors are publicly exported. Preserve compatibility entrypoints and child-token/shared-control identity; do not silently break their Rust interface.
- **ResolvedStreamerContext:** it currently wraps `Arc<MergedConfig>`. Collapsing the wrapper can simplify caching, but the type and methods are public. Preserve compatibility rather than treating direct removal as an internal cleanup.

## Validation and implementation boundaries

Implement small simplifications separately from correctness fixes and scheduling policy changes. For Rust changes, run formatting, affected-package Clippy, and focused contract tests under the repository's guidance. Broaden checks when a public interface changes. Any new index needs a new migration, never an edit to an applied one.

Do not remove committed-write ownership, generation/revision checks, fresh configuration reads, filesystem alias checks, transactional retention rechecks, or partial-failure lookup recovery solely because they add code. These protect observable contracts. Do not replace deliberately different processor policies with a universal runner.

Existing protections already include cached health sampling, shared WebSocket encoding, bounded processor logs, adaptive idle polling, batched related lookups/log writes, a combined SQLite page-cache budget, buffered danmu writes, bounded aggregation, and checkpoint CPU admission. They were inspected and are not missing optimizations.

The proposed per-offline-poll `Success(None)` write amplification was rejected after tracing production callers. It should not be carried forward as a finding.

External contract references: [Rust copy retries Interrupted](https://doc.rust-lang.org/std/io/fn.copy.html); [Tokio blocking task cancellation](https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html); [Tokio filesystem implementation model](https://docs.rs/tokio/latest/tokio/fs/index.html); [Reqwest idle-connection limit](https://docs.rs/reqwest/latest/reqwest/struct.ClientBuilder.html#method.pool_max_idle_per_host); [SQLite query-plan interpretation](https://sqlite.org/eqp.html).
