# Data Governance

Rust-Srec is a recording tool, not a source of rights to record or process a broadcast. The operator is responsible for platform terms, copyright, privacy, employment, and retention obligations in every applicable jurisdiction.

## Data Inventory

| Data | Typical location | Risk |
|---|---|---|
| Video, audio, and chat/danmaku | `OUTPUT_DIR` and pipeline destinations | Copyrighted content, personal data, usernames, messages |
| Streamer and session metadata | SQLite in `DATA_DIR` | Identifiers, URLs, titles, activity history |
| Platform credentials and cookies | Database/configuration and exports | Account takeover and access to restricted content |
| User accounts and password hashes | SQLite and configuration exports | Authentication data |
| Notification and upload secrets | SQLite and configuration exports | Third-party account or endpoint access |
| Logs and notification events | `LOG_DIR`, container logs, database | Paths, platform metadata, operational history |

## Required Decisions Before Use

- Document the lawful basis and permission for each channel, including private, subscriber-only, or password-protected content.
- Define retention separately for media, chat, session history, job history, notifications, and logs.
- Identify every external destination used by webhooks, email, Telegram, Gotify, Discord, rclone, Baidu Netdisk (BaiduPCS-Go), or custom pipeline commands.
- Limit access to people with a defined operational or editorial need.
- Define how deletion, legal hold, subject requests, and incident response apply to source files, derived files, backups, and remote copies.

Application history-retention settings do not constitute a complete deletion policy and do not remove every media copy. Pipelines may create derivatives or upload data beyond the host. Verify deletion end to end.

## Auditability Limits

Sessions, pipeline jobs, notification events, and logs are operational tools, not compliance evidence. If you need an audit trail, legal hold, or data-subject request handling, see [Scope and Limits](./support.md#scope-and-limits) and add those controls externally.

## Decommissioning

Disable streamers, revoke platform and notification credentials, revoke user sessions, stop the service, inventory remote outputs, then erase or archive data according to policy. Include configuration exports and backups; deleting the live database alone is insufficient.
