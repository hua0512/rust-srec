# Storage and Capacity

Recordings are usually the dominant cost of a Rust-Srec deployment. Capacity planning must include raw recordings, chat files, pipeline intermediates, final artifacts, and the space needed during backup or migration.

## Estimate Recording Volume

A practical decimal estimate is:

```text
GB per hour = average bitrate in Mbit/s x 0.45
Daily GB = GB per hour x hours recorded per day x simultaneous streams
Required capacity = daily GB x retention days x safety factor
```

At 6 Mbit/s, one stream uses about 2.7 GB per hour. Eight hours per day for 30 days is about 648 GB before chat files, pipeline outputs, filesystem overhead, and safety margin. Measure actual platform bitrates; transcoding may reduce final size while temporarily increasing peak usage.

Use a safety factor of at least 1.2, and more when streams are unpredictable or pipelines keep both source and output files.

## Docker Paths

| Host setting | Container path | Contents |
|---|---|---|
| `DATA_DIR` | `/app/data` | SQLite database and application data |
| `CONFIG_DIR` | `/app/config` | Platform configuration files |
| `OUTPUT_DIR` | `/app/output` | Recordings and configured pipeline output |
| `LOG_DIR` | `/app/logs` | Application logs |

Use absolute host paths in production. Make sure the container user can create, rename, and delete files in every configured output root. Do not point `OUTPUT_DIR` at the operating system volume unless its capacity and alerts are intentionally shared.

## System Service Paths

An installation running under the bundled `rust-srec.service` unit uses these instead:

| Path | Contents |
|---|---|
| `/var/lib/rust-srec` | SQLite database, WAL files, and the runtime generation marker; also the service account's home directory |
| `/var/lib/rust-srec/output` | Recording volume the unit points `OUTPUT_DIR` and `RUST_SREC_OUTPUT_ROOTS` at |
| `/var/log/rust-srec` | Daily-rotated application log |

systemd creates all three on every start, owned by the service account, so a fresh host needs nothing pre-created.

Give a recordings tree that will grow past a few hundred gigabytes its own volume, and list that volume under `ReadWritePaths=` instead of nesting it below `/var/lib/rust-srec`. `StateDirectory=` recursively chowns everything under the state directory whenever it finds it owned by another user, and that walk runs during unit start — on a large tree it can outlast the start timeout. A path listed under `ReadWritePaths=` must already exist and be owned by the service account; systemd neither creates nor chowns it.

The recording directory comes from `output_folder` in the database. The standalone backend initializes it from `OUTPUT_DIR` only for a fresh database; later starts preserve the saved value. Discovery uses saved settings and overrides, rather than treating an obsolete `OUTPUT_DIR` as another recording location. Keep explicitly configured `RUST_SREC_OUTPUT_ROOTS` aligned with the saved folder. See [Environment Variables](../getting-started/configuration.md#environment-variables).

## Output-Root Probes

Startup discovery uses actual streamer and platform names to select concrete
directories that map to the same gate keys as recording attempts. A probe tests
a concrete configured directory, which can be writable even when its ancestor
gate key is read-only. Explicit `RUST_SREC_OUTPUT_ROOTS` boundaries take priority.
Templates whose early title, session, or date placeholders make the key uncertain
are skipped; explicit boundaries can provide probe coverage for those layouts.

Probe targets are grouped by gate key, using a deterministic concrete representative.
Startup attempts cover at most 16 keys, with at most four attempts awaiting results
at once; skipped targets are logged. An operating-system call can outlive its
five-second wait, but total startup probe launches remain bounded. Sampling one
representative cannot certify permissions in every child directory. Each actual
recording still checks its own output path.

Disk probes are registered at startup. Changing output settings does not rebuild
that probe set, and historical gate entries for roots no longer used are retained.
These limits do not change the key used by a new recording attempt.

## Capacity Controls

- Set `max_concurrent_downloads` to cap simultaneous writers.
- Split long recordings with duration or part-size limits if downstream systems cannot handle very large files.
- Keep CPU and IO pipeline concurrency below the sustained capacity of the host.
- Configure job and notification-event history retention separately; those settings do not delete media files.
- Use a tested pipeline move/upload step only after verifying the destination and delete-source behavior on noncritical data.

Deleting a streamer reclaims nothing. Its sessions and the media-output rows, segments, and danmaku statistics recorded against them are kept on purpose. To reclaim space, delete the media outputs you no longer need with the option to remove the file from disk, then delete the sessions. That order matters: deleting a session also removes its media-output rows, and those rows hold the only paths the application has to the files. See [Deletion Semantics](./data-governance.md#deletion-semantics).

Rust-Srec does not replace an organizational media lifecycle policy. Automate media deletion or archival outside the application if your retention requirement demands it, and protect that automation against deleting active files.

## Alerts and Failure Handling

Monitor free bytes and free percentage on the output volume. Alert early enough to cover the longest expected active recording plus pipeline temporary space. The output-root write gate can stop new work and emit a critical notification when a configured root becomes unwritable; it is not a substitute for capacity forecasting.

When output fails:

1. Stop adding new work and confirm which root is affected.
2. Check free space, inode/file-count limits, mount state, and ownership.
3. Restore write access and verify a small test write on the same filesystem.
4. After the gate's cooldown, allow an affected recording to retry its real directory check, then confirm the System Health page recovers. A retained entry for a root no longer used cannot be healed by an attempt writing elsewhere.
