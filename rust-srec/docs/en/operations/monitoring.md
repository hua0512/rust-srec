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

### Slow Filesystem Sampling

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

Application retention runs when the cleanup service starts and then daily,
removing recognized dated log files older than seven days. It does not currently
bound bytes per file. File initialization failures return a startup error.
Console colors are enabled only when stdout is a terminal, and live-log event
formatting is skipped when no client is subscribed.

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
