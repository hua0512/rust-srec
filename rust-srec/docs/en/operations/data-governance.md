# Data Governance

Rust-Srec is a recording tool, not a source of rights to record or process a broadcast. The operator is responsible for platform terms, copyright, privacy, employment, and retention obligations in every applicable jurisdiction.

## Data Inventory

| Data | Typical location | Risk |
|---|---|---|
| Video, audio, and chat/danmaku | `OUTPUT_DIR` and pipeline destinations | Copyrighted content, personal data, usernames, messages |
| Streamer and session metadata | SQLite in `DATA_DIR` | Identifiers, URLs, titles, session history, per-check history. Every session also stores the streamer name it was recorded under, and that copy is kept after the streamer itself is deleted; per-check history is not. |
| Platform credentials and cookies | Database/configuration and exports | Account takeover and access to restricted content |
| User accounts and password hashes | SQLite and configuration exports | Authentication data |
| Notification and upload secrets | SQLite and configuration exports | Third-party account or endpoint access |
| Logs and notification events | `LOG_DIR`, container logs, database | Paths, platform metadata, operational history |

## Required Decisions Before Use

- Document the lawful basis and permission for each channel, including private, subscriber-only, or password-protected content.
- Define retention separately for media, chat, session history, job history, notifications, and logs. Session history has a lifetime of its own: it is not bounded by how long the streamer that produced it exists. Per-check history is the exception — it is deleted with its streamer.
- Identify every external destination used by webhooks, email, Telegram, Gotify, Discord, rclone, Baidu Netdisk (BaiduPCS-Go), or custom pipeline commands.
- Limit access to people with a defined operational or editorial need.
- Define how deletion, legal hold, subject requests, and incident response apply to source files, derived files, backups, and remote copies. Name the operation that satisfies each of them; see [Deletion Semantics](#deletion-semantics) for what removing a streamer does and does not cover.

Application history-retention settings do not constitute a complete deletion policy and do not remove every media copy. Pipelines may create derivatives or upload data beyond the host. Verify deletion end to end.

## Deletion Semantics

Deleting a streamer removes that streamer and its configuration. It deliberately keeps the recording history: the streamer's sessions survive, as do the media-output rows, session segments, danmaku statistics, and session events attached to them. A surviving session is marked ended and carries the streamer name it was recorded under, so the history stays readable without the streamer. That name is written when the session starts and is never updated afterwards, so a session that began before a rename keeps the older name — and a session whose streamer was already missing carries no name at all.

Two kinds of record do not come through intact:

- Per-check history — the outcome, selected stream, title, and viewer count recorded for each status check — is deleted with the streamer.
- Notification events survive, but their reference to the streamer is cleared and no name is stored alongside it, so those rows lose their attribution. Take this into account when setting notification retention.

For a retention or erasure policy this means removing a streamer is not the erasure step, and the retained name outlives its subject. Three further operations are needed, and the first two have to run in this order:

- Delete the media files first, while the records naming them still exist. Deleting a media output takes an option to remove the file from disk as well as its record; without it, only the record goes.
- Delete the sessions themselves, individually or as a batch. A session delete also removes the media-output rows, segments, danmaku statistics, and session events under it. It does not touch files on disk, and it destroys the rows holding their paths, so running it first leaves the files with nothing left to locate them by. Pipeline jobs, DAG executions, and upload records reference a session by identifier without a foreign key, so they survive it.
- Account separately for copies a pipeline uploaded elsewhere. No deletion operation in the application reaches a remote destination — the pipeline's `delete` step and the media-output delete option both act on a local path — so remote copies have to be removed wherever they were sent. Upload records carry the remote path of each completed upload and are the closest thing to an inventory of them.

A pipeline can still change remote data, just not as part of a deletion the application offers: an `rclone` step set to `sync` removes files at the destination that are absent from the source, `args` passes arbitrary flags through to rclone, and an `execute` step runs whatever command it is given. Treat those steps as having the full reach of the credentials they run with.

Two narrow cases delete a file without being asked to: a recorded segment below `min_segment_size_bytes` is removed from disk with its sibling chat file while the session is still running, and a failed pipeline job's partial outputs are cleaned up. Neither substitutes for a retention policy.

## Auditability Limits

Sessions, pipeline jobs, notification events, and logs are operational tools, not compliance evidence. If you need an audit trail, legal hold, or data-subject request handling, see [Scope and Limits](./support.md#scope-and-limits) and add those controls externally.

## Decommissioning

Disable streamers, revoke platform and notification credentials, revoke user sessions, stop the service, inventory remote outputs, then erase or archive data according to policy. Neither disabling nor deleting streamers is a purge: recording history and the streamer names stored on it survive a streamer delete, and no delete removes a media file unless it is a media-output delete asked to remove the file too. Include configuration exports and backups; deleting the live database alone is insufficient.
