# Monitoring

Monitor the process, its dependencies, the host, and recording outcomes. A running container alone does not prove that the database is ready, an output volume is writable, or a platform can be recorded.

## Health Endpoints

| Endpoint | Authentication | Use |
|---|---|---|
| `GET /api/health/live` | Public | Process liveness and uptime; used by the Docker health check |
| `GET /api/health/ready` | Bearer token when auth is enabled | Returns `200 ready` or `503 not ready` based on component health |
| `GET /api/health` | Bearer token when auth is enabled | Version, uptime, component status, CPU usage, and memory usage |

Use liveness to restart a dead process. Authenticated readiness rejects unknown startup state and unhealthy instances; degraded instances remain ready. Alert separately on degraded component health. Protect the token used by the monitor and give the monitoring network only the access it needs.

After a refresh, any component whose status is unknown makes the overall health
degraded, unless another component is unhealthy. For example, a disk that cannot
be resolved cannot produce a healthy overall report. The initial snapshot remains
unknown until the first refresh.

```bash
curl http://localhost:12555/api/health/live
curl http://localhost:12555/api/health/ready \
  -H "Authorization: Bearer <access-token>"
```

There is no `/metrics` endpoint. Scrape the JSON health endpoints above, or collect from outside the application.

The internal delivery collector tracks web-push outcomes only. Download,
pipeline and scheduler information comes from their dedicated API snapshots;
these are not a Prometheus export.

### Startup Output Paths

Disk-health paths are discovered at startup, with at most 16 initial write-probe targets, four probes at once, and a five-second wait per probe. This is not a continuously refreshed inventory. Changes to output settings do not rebuild the probe list; check the actual recording path and see [output-root probes](./storage.md#output-root-probes).

### Slow Filesystem Sampling

If filesystem sampling takes longer than one second, health checks retain previous CPU, memory, and disk values and report degraded sampling. Other probes continue. Treat those figures as stale and inspect unavailable network or FUSE mounts. Sampling resumes only when the blocked operating-system call returns.

## SQLite Memory Budget

Database connections share a suggested 64 MiB private page-cache allowance. File-backed databases also use a 256 MiB memory-mapping setting. Neither is a hard process-memory limit: recordings, queries, connection overhead, and operating-system caching also use memory. No new environment setting is required. Allocation details are in [Monitoring implementation](../development/monitoring.md#sqlite-memory-budget).

## Logs

In Docker:

```bash
docker compose logs --since=30m rust-srec
docker compose logs --since=30m frontend
```

With the systemd service:

```bash
journalctl -u rust-srec --since "30 min ago"
journalctl -u rust-srec -f
```

The example Compose file rotates container JSON logs. The unit sets `StandardOutput=journal` and `StandardError=journal` with `SyslogIdentifier=rust-srec`, so process output goes to the journal and is subject to the host's journald retention.

In both deployments the application also writes its own daily-rotated log files to `LOG_DIR`, which the unit points at `/var/log/rust-srec`. Centralize logs when incident history must survive host loss, and filter access because paths and platform metadata may be sensitive even though credentials are redacted by the application.

Application files rotate at UTC date changes and before a write exceeds
`LOG_MAX_FILE_BYTES` (16 MiB by default). `LOG_MAX_FILES` defaults to 16 and includes
the current segment. Oldest managed segments are removed to make room, so seven
days is a maximum age, not a guaranteed retention window. Startup and daily
cleanup also remove expired or oversized legacy segments. Oversized individual
records retain a UTF-8 prefix and a truncation marker; stderr reports cumulative
truncation and write-failure counts at increasing intervals.

Legacy `rust-srec.log.YYYY-MM-DD` files remain readable and can be appended while
they fit. New segments use `rust-srec.log.YYYY-MM-DD.<20-digit sequence>`. Listing,
date filters and archives use the filename's UTC date for both forms. Rotation
never truncates existing files. Archives retain their scanned-length snapshot;
if retention removes a selected file before it is opened, the download fails
explicitly rather than silently omitting that file.

Independent instances may share `LOG_DIR`: a short file lock coordinates each
append, rotation and cleanup. Keep their limits consistent. With matching limits,
successful maintenance bounds managed named files to 16 × 16 MiB by default;
unrelated names, symlinks, filesystem overhead and deleted files still held open
by readers are outside that bound. Do not remove `.rust-srec.log.lock` while any
instance is running; it also stores the rotation counter. Uncoordinated external
writers or older application versions sharing the directory cannot honor these
limits. If capacity cannot be reclaimed, file writes fail instead of exceeding
the limits; console and live logging remain independent.

Invalid limit settings or file initialization failures return a startup error.
Redirected console output omits terminal colors.

## Scheduler Restart Limits

An actor that repeatedly crashes stops restarting after its tenth consecutive
crash. This count does not expire with the 60-second restart-backoff window;
ordinary platform-check failures handled inside the actor do not count as actor
crashes. A removed streamer or another non-recoverable actor decision stops
without scheduling a restart.

Investigate an `exceeded restart limit` log before restoring monitoring. After
fixing the cause, disable and re-enable the streamer to remove its old actor
tracking and start with a fresh crash budget, or restart the service. Waiting
for the backoff window alone does not restore an exhausted budget.

## Minimum Alert Set

- Backend liveness failure or restart loop.
- Readiness remains non-200 beyond the expected startup period.
- Output volume free space or inode count crosses warning and critical thresholds.
- Output root becomes unwritable.
- Recording or segment failures increase for one or many platforms.
- Pipeline queue age, failures, or CPU/IO saturation increase.
- Credentials expire or platform authentication starts failing.
- Backup age or restore-check age exceeds policy.

Configure at least two paths for critical alerts when a single notification provider is also a dependency at risk.

## Operational Review

Review the **System Health**, **Sessions**, **Pipeline Jobs**, and **Notification Events** pages. Investigate changes in outcomes, not only infrastructure: an always-offline channel, repeated short segments, or an empty output file can indicate a failure while the process remains healthy.
