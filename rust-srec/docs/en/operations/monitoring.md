# Monitoring

Monitor the process, its dependencies, the host, and recording outcomes. A running container alone does not prove that the database is ready, an output volume is writable, or a platform can be recorded.

## Health Endpoints

| Endpoint | Authentication | Use |
|---|---|---|
| `GET /api/health/live` | Public | Process liveness and uptime; used by the Docker health check |
| `GET /api/health/ready` | Bearer token when auth is enabled | Returns `200 ready` or `503 not ready` based on component health |
| `GET /api/health` | Bearer token when auth is enabled | Version, uptime, component status, CPU usage, and memory usage |

Use liveness to restart a dead process. Use authenticated readiness to stop routing work to a degraded instance and alert an operator. Protect the token used by the monitor and give the monitoring network only the access it needs.

```bash
curl http://localhost:12555/api/health/live
curl http://localhost:12555/api/health/ready \
  -H "Authorization: Bearer <access-token>"
```

There is no `/metrics` endpoint. Scrape the JSON health endpoints above, or collect from outside the application.

## Logs

```bash
docker compose logs --since=30m rust-srec
docker compose logs --since=30m frontend
```

The example Compose file rotates container JSON logs. The application also writes to `LOG_DIR`. Centralize logs when incident history must survive host loss, and filter access because paths and platform metadata may be sensitive even though credentials are redacted by the application.

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
