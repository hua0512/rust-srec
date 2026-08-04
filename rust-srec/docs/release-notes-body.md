## rust-srec v0.5.1

A small release. The **Hooks** tab is gone — its shell commands were stored and merged like any other setting but were never actually run — and notification channels cover what they were meant to do. Two streamer fixes land alongside.

### Highlights
- **Event hooks removed in favor of notification channels** — the **Hooks** tab on a site, a template, a template's per-site override, and a streamer is gone, along with the six shell commands it held. To act on these moments, subscribe a notification channel (webhook, Telegram, Gotify, Discord, or email) under **Notifications**.
- **Bigo streamers are picked up as soon as they go live** — Bigo grants an access token a single use, so checks that shared one token got a reply without the stream's address and read it as "not live". Each check now takes its own token, and a missing address is reported as a failed check rather than an offline room.
- **Turning a streamer off clears its error history** — switching a streamer off wipes its recorded error, its consecutive-error count, and any retry backoff still counting down, so switching it back on starts checking right away instead of sitting out the rest of the wait.

### Review before upgrading
- **Saved hook commands are deleted on upgrade.** They are removed from the database and no longer appear in an exported backup; copy anything you want to keep before upgrading. Since the commands were never executed, nothing you were relying on stops working.

Full release notes: https://docs.srec.rs/en/release-notes/v0.5.1 · 中文版：https://docs.srec.rs/zh/release-notes/v0.5.1
