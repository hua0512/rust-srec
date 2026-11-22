# Monitoring implementation

For contributors changing sampling, database cache allocation, and log rotation. Operational checks are in [Monitoring](../operations/monitoring.md).

## Startup Output Paths {#startup-output-paths}

Startup discovers and caps the output-root paths once, then shares that exact
snapshot between disk-health registration and the one-shot write probes. Limits: at most 16 write-probe targets, four probes at a time, and a
five-second timeout per probe. Later configuration changes rely on actual
output writes for write-gate feedback; the startup snapshot is not a live inventory.

## Slow Filesystem Sampling {#slow-filesystem-sampling}

System and disk metrics are sampled on one dedicated thread. Health refreshes
wait up to one second for a sample, then retain the last available CPU, memory,
and disk values and report degraded system sampling. Other due probes continue.
Retained values can be stale; use the component status and message when deciding
whether disk figures are current.

A stalled sample stays the only operation in flight. The service does not launch
replacement threads on each timeout. Health reads and health-checker cancellation
remain responsive, but sampling can resume only after the operating-system call
returns. A permanently blocked sampler thread may remain until process exit;
inspect unavailable network or FUSE mounts on the host.

## SQLite Memory Budget {#sqlite-memory-budget}

The standard database pools share a 64 MiB suggested private page-cache budget:
56 MiB is divided by the configured maximum number of read connections, rounding
down to whole KiB, and 8 MiB is reserved for the single serialized writer. With
10 readers, each uses `PRAGMA cache_size = -5734`; the writer uses `-8192`.
The adaptive read-pool limit is twice the available CPU count, capped at 10.
Rust callers of `init_pool_with_size` share the same reader allowance across their
requested limit; opening the write pool separately uses the reserved writer allowance.

Small private caches can cause more page reads for large working sets, which are
served through the operating system's file cache. Per-connection `mmap_size` is
disabled (`0`) so large database files do not map hundreds of megabytes per pool
connection into the process resident set outside the page-cache budget. SQLite
treats `cache_size` as a suggestion; this is not a hard limit on database or process
memory. Temporary queries, connection overhead and the recorder's other
services also contribute to memory use.

## Logs {#logs}

Operational log commands, retention limits, and segment naming live in [Monitoring: Logs](../operations/monitoring.md#logs). Contributor-facing implementation invariants:

- **Cross-instance coordination**: independent instances sharing `LOG_DIR` coordinate each append, rotation, and cleanup through the advisory file lock `.rust-srec.log.lock`, which also stores the rotation counter. Uncoordinated external writers or older application versions sharing the directory cannot honor these bounds.
- **Abort-build emergency panic fallback**: emergency panic writes in `panic = "abort"` builds attempt to acquire the same lock without waiting and fall back to `stderr` if the lock cannot be acquired or the write fails.
- **Console color detection**: ANSI colors on console output are enabled only when `stdout` is a terminal (TTY).
- **Lazy live-log formatting**: live-log WebSocket event formatting is skipped whenever no client is subscribed.
