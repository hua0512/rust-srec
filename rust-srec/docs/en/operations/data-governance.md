# Data Governance

Rust-Srec is a recording tool, not a source of rights to record or process a broadcast. The operator is responsible for platform terms, copyright, privacy, employment, and retention obligations in every applicable jurisdiction.

## Data Inventory

| Data | Typical location | Risk |
|---|---|---|
| Video, audio, and chat/danmaku | `OUTPUT_DIR` and pipeline destinations | Copyrighted content, personal data, usernames, messages |
| Streamer and session metadata | SQLite in `DATA_DIR` | Identifiers, URLs, titles, activity history. Every session also stores the streamer name it was recorded under, and that copy is kept after the streamer itself is deleted. |
| Platform credentials and cookies | Database/configuration and exports | Account takeover and access to restricted content |
| User accounts and password hashes | SQLite and configuration exports | Authentication data |
| Notification and upload secrets | SQLite and configuration exports | Third-party account or endpoint access |
| Logs and notification events | `LOG_DIR`, container logs, database | Paths, platform metadata, operational history |

## Required Decisions Before Use

- Document the lawful basis and permission for each channel, including private, subscriber-only, or password-protected content.
- Define retention separately for media, chat, session history, job history, notifications, and logs. Session history has a lifetime of its own: it is not bounded by how long the streamer that produced it exists.
- Identify every external destination used by webhooks, email, Telegram, Gotify, Discord, rclone, Baidu Netdisk (BaiduPCS-Go), or custom pipeline commands.
- Limit access to people with a defined operational or editorial need.
- Define how deletion, legal hold, subject requests, and incident response apply to source files, derived files, backups, and remote copies. Name the operation that satisfies each of them; see [Deletion Semantics](#deletion-semantics) for what removing a streamer does and does not cover.

Application history-retention settings do not constitute a complete deletion policy and do not remove every media copy. Pipelines may create derivatives or upload data beyond the host. Verify deletion end to end.

## Deletion Semantics

Deleting a streamer removes that streamer and its configuration. It deliberately keeps the recording history: the streamer's sessions survive, as do the media-output rows, session segments, danmaku statistics, and session events attached to them. Each surviving session is marked ended and is labelled with the streamer name it carried at the time, so the history stays readable without the streamer.

For a retention or erasure policy this means removing a streamer is not the erasure step, and the retained name outlives its subject. Three further operations are needed:

- Delete the sessions themselves, individually or as a batch, to remove a session and everything recorded against it. This clears the records; it does not touch the files on disk.
- Delete the media files. Deleting a media output takes an option to remove the file from disk as well as its record; without it, only the record goes. Nothing deletes a file implicitly, so a policy has to say which of the two is intended.
- Account separately for copies a pipeline uploaded elsewhere. No operation in the application reaches a remote destination, so those have to be removed wherever they were sent.

## Auditability Limits

Sessions, pipeline jobs, notification events, and logs are operational tools, not compliance evidence. If you need an audit trail, legal hold, or data-subject request handling, see [Scope and Limits](./support.md#scope-and-limits) and add those controls externally.

## Decommissioning

Disable streamers, revoke platform and notification credentials, revoke user sessions, stop the service, inventory remote outputs, then erase or archive data according to policy. Neither disabling nor deleting streamers is a purge: recording history and the streamer names stored on it survive a streamer delete, and no database operation removes media files. Include configuration exports and backups; deleting the live database alone is insufficient.
