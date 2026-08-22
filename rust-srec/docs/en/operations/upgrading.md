# Upgrade and Rollback

Upgrade the backend and frontend together. Database migrations run at startup and are not reversible, so a pre-upgrade snapshot is your rollback boundary.

## Before Upgrading

1. Read every release note between the current and target versions.
2. Confirm the target image tag exists for both `rust-srec` and `rust-srec-frontend`.
3. Download a configuration export and take a consistent backup of the database, configuration, output, `.env`, and Compose file.
4. Record the current image tags or digests and the result of health checks.
5. Schedule around active recordings and pipeline jobs. An interrupted media write may need manual inspection.

## Docker Upgrade

Edit `.env` so `VERSION` is the reviewed target, for example:

```dotenv
VERSION=v0.5.1
```

Then run:

```bash
docker compose pull
docker compose up -d
docker compose ps
docker compose logs --tail=200 rust-srec
curl http://localhost:12555/api/health/live
```

After signing in, verify authenticated readiness, the System Health page, streamer count, recent sessions, one platform check, and a noncritical recording/pipeline. Keep the pre-upgrade backup until the observation period ends.

## Automatic Updates (Watchtower)

The Compose file ships an optional `watchtower` service that pulls new images and recreates the containers automatically. It is off by default; enable it with:

```bash
docker compose --profile autoupdate up -d
```

Updates only happen while the system is idle. Before stopping a container, Watchtower runs its pre-update hook, which calls the unauthenticated `GET /api/health/idle` endpoint:

- `200` — nothing is recording or queued to record, and no pipeline job (upload, remux, danmaku conversion, ...) is processing. The update proceeds.
- `503` or no response — the hook exits with code 75 and Watchtower skips this cycle, retrying on the next poll (`WATCHTOWER_POLL_INTERVAL`, default 3600 seconds).

Pending pipeline jobs do not block an update; they are persisted and re-run after the restart. The frontend container carries the same gate, so both images move to the new version in the same cycle.

Caveats:

- Automatic updates require a mutable image tag: keep `VERSION=latest` (or `dev`). Pinned `vX.Y.Z` tags never receive updates.
- Database migrations still run at startup and are not reversible. Automatic updates skip the manual pre-upgrade snapshot, so keep scheduled backups (see [Backup and Restore](./backup-restore.md)) and read release notes regularly. If an upgrade must be reviewed before rollout, stay on manual upgrades with a pinned tag.
- The idle check runs immediately before the container stops; a recording that starts in the few seconds in between is still interrupted.

## Rollback

Do not start an older binary against a database already migrated by a newer release unless the release notes explicitly say it is compatible.

1. Stop both services.
2. Restore the pre-upgrade database and configuration snapshot.
3. Reset `VERSION` to the previous tag or image digest.
4. Start both services and repeat the health and recording checks.

Any sessions or configuration created after the snapshot will be lost. If those changes matter, preserve the failed-upgrade data separately for forensic recovery rather than merging it directly into the restored database.

## Source or Binary Installations

Keep the exact binary, frontend build, environment file, and database snapshot as one release unit. Build with `--locked`; do not combine an arbitrary frontend commit with a different backend release.
