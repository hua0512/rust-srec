# Backup and Restore

Use two backup layers. A configuration export is portable and convenient, while a filesystem snapshot is required to recover operational history and media.

## What Each Backup Contains

| Backup | Includes | Does not include |
|---|---|---|
| **Settings > Backup & Restore** export | Global settings, templates, streamers and filters, engines, platform settings, notification channels/subscriptions, job and pipeline presets, users and password hashes, credential profiles and selection policies | Recording media, session/job history, logs, refresh-token sessions |
| Filesystem backup | Whatever you copy from `DATA_DIR`, `CONFIG_DIR`, `OUTPUT_DIR`, and optionally `LOG_DIR` | External upload destinations and notification services |

::: warning Sensitive Export
The configuration export can contain platform cookies, profile refresh/access tokens and re-login material, notification credentials, channel settings, user metadata, and password hashes. Encrypt it, restrict access, and do not attach it to a public issue.
:::

## Configuration Export

In the web interface, open **Settings > Backup & Restore** and download an export. The API equivalents are `GET /api/config/backup/export` and `POST /api/config/backup/import`.

Filter and notification-subscription lookups use batches of up to 500 owners,
preserving streamer/channel order and each owner's existing child order. If a
batch read fails, the backend retries those owners individually so healthy
neighbors remain in the export. As before, an owner whose related-data lookup
still fails has an empty filter/subscription list; the backend logs the recovery
and failure counts. Failures to load the main configuration lists still abort the export.

Import supports two modes:

- `merge` updates matching entities and keeps entities absent from the file.
- `replace` removes existing managed configuration not present in the import. Treat this as destructive and test it on a disposable instance first.

For exports using schema version `0.1.3` or later with a nonempty user list,
users are matched by username. Email uniqueness is checked against the final
user set before any configuration writes, including accounts retained by `merge`.
Email comparisons preserve case and whitespace: `User@example.com` and
`user@example.com` are distinct. An empty string is a unique value; multiple
users may have no email (`null`).

An import may swap emails between updated users or assign an email released by
another updated user. In `replace` mode it may reuse an omitted user's email.
Existing users keep their IDs and creation dates when their usernames match.
If any later import write fails, email changes and other configuration writes
roll back together. An omitted or empty user list, or a schema older than `0.1.3`,
leaves existing users unchanged in either mode.

Every successful import revokes all refresh tokens and their bound access-token
sessions, including a `merge` that omits users. Sign in again after import.
Rejected imports do not revoke tokens.

An export is useful for migration and source-controlled review after secrets are removed, but it is not a database backup.

Imports apply configuration as one database transaction. Validation or write failures before commit leave it unchanged. A reload warning after commit means the configuration was saved, but runtime refresh needs attention.

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

<div id="import-persistence-ownership" class="legacy-section">

This section is now in [Persistence contracts](../development/persistence.md#import-persistence-ownership).

</div>

## Credential profiles and rollback {#credential-profiles-and-rollback}

An export containing only legacy credentials keeps schema `0.1.8` and its existing JSON shape. Any saved profile or explicit selection policy, including `inherit` or `none` with no profiles, requires schema `1.0.0`. Older importers reject that version. This is a backup format version, not the application version. A filtered export must retain referenced profiles, their owners and platforms; incomplete credential graphs are rejected. Profiles contain cookies, refresh/access tokens and supported re-login material, so protect them like the live database.

Profile UUIDs are preserved when free. An existing UUID is updated only if its resolved owner and platform match; otherwise the entire import fails. Accounts are never guessed from labels or cookie contents. Owners are resolved using platform names, template names and streamer URLs. Repeated imports update the same accounts. All profile/reference validation and configuration writes share one transaction. A legacy-format import cannot overwrite converted authentication fields or remove managed policies.

Merge retains omitted profiles and omitted selection policies; use an explicit `inherit` policy to reset a selection. Replace retires omitted profiles and removed owners, stops affected recordings after commit, and retains material until active sessions settle before physical deletion. Runtime publication continues even if the HTTP client disconnects. Restored material receives a new revision so old refresh results cannot overwrite it; later download attempts obtain fresh media. Managed profile health, cooldowns, round-robin positions, session bindings, QR receipts and playback contexts are not configuration-backup data.

Before upgrading or converting, take a consistent database backup. Running an older binary against an upgraded database is not a supported rollback: it may reject migration versions and cannot interpret managed policies. Restore the matching pre-upgrade database backup with the previous binary. There is no lossless pool-to-scalar downgrade export.
