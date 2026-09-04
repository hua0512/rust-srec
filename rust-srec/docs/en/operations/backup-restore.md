# Backup and Restore

Use two backup layers. A configuration export is portable and convenient, while a filesystem snapshot is required to recover operational history and media.

## What Each Backup Contains

| Backup | Includes | Does not include |
|---|---|---|
| **Settings > Backup & Restore** export | Global settings, templates, streamers and filters, engines, platform settings, notification channels/subscriptions, job and pipeline presets, users and password hashes | Recording media, session/job history, logs, refresh-token sessions |
| Filesystem backup | Whatever you copy from `DATA_DIR`, `CONFIG_DIR`, `OUTPUT_DIR`, and optionally `LOG_DIR` | External upload destinations and notification services |

::: warning Sensitive Export
The configuration export can contain platform cookies, notification credentials, channel settings, user metadata, and password hashes. Encrypt it, restrict access, and do not attach it to a public issue.
:::

## Configuration Export

In the web interface, open **Settings > Backup & Restore** and download an export. The API equivalents are `GET /api/config/backup/export` and `POST /api/config/backup/import`.

Import supports two modes:

- `merge` updates matching entities and keeps entities absent from the file.
- `replace` removes existing managed configuration not present in the import. Treat this as destructive and test it on a disposable instance first.

An export is useful for migration and source-controlled review after secrets are removed, but it is not a database backup.

## Consistent Filesystem Backup

For the standard Docker layout:

1. Download a fresh configuration export.
2. Disable new recording work or schedule a maintenance window.
3. Run `docker compose stop` so SQLite and active media files are consistent.
4. Snapshot or copy `DATA_DIR`, `CONFIG_DIR`, `OUTPUT_DIR`, the `.env` file, and `docker-compose.yml`. Include `LOG_DIR` only when required by your incident-retention policy.
5. Restart with `docker compose up -d` and verify liveness.

With the systemd service the sequence is the same, against the unit's own paths:

1. Download a fresh configuration export.
2. Disable new recording work or schedule a maintenance window.
3. Run `systemctl stop rust-srec` so SQLite and active media files are consistent.
4. Snapshot or copy `/var/lib/rust-srec` (database, WAL files, and the default output directory), `/etc/rust-srec/rust-srec.env`, and any recording volume listed under `ReadWritePaths=`. Include `/var/log/rust-srec` only when required by your incident-retention policy.
5. Run `systemctl start rust-srec` and verify liveness.

Protect `.env`, `/etc/rust-srec/rust-srec.env`, and backup media with the same or stronger controls as the live service. Keep at least one backup outside the host and test its integrity.

## Restore Drill

1. Provision a clean host with enough space and the same pinned Rust-Srec version.
2. Keep the service stopped while restoring the saved directories to the same absolute paths and permissions.
3. Restore `.env` and Compose configuration, or `/etc/rust-srec/rust-srec.env` and the unit file. Do not generate a new `JWT_SECRET` during a like-for-like restore unless invalidating all access tokens is intended.
4. Start the service and check `/api/health/live`.
5. Sign in, check authenticated `/api/health/ready`, inspect streamers and sessions, and play or checksum representative media.
6. Test one noncritical live recording and its pipeline.

A session can outlive the streamer it was recorded for: `live_sessions.streamer_id` is nullable, and deleting a streamer keeps the session, its media rows, and the streamer name stored on the session. Sessions with no matching streamer in a restored database are expected in that case and are not evidence of a damaged restore.

Record the restore time and the point-in-time loss observed. Those measurements are your actual recovery time and recovery point.
