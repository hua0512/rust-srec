## rust-srec v0.5.0

A feature release centred on setting streamers up and seeing what the app is doing, plus a broad reliability pass over recording, monitoring, pipelines, the database, and the web interface.

### Highlights
- **Rebuilt add and edit streamer pages** — adding a streamer asks for the link first and confirms the site is recognized before anything else, and the edit page keeps settings, recording history, and recent sessions in one place. Leaving with unsaved changes now asks first.
- **The site is worked out from the link** — replacing a streamer's link with one from a different site now moves it across properly instead of leaving it on the old site's cookies, proxy, and output folder. It is no longer something you pick by hand.
- **Per-streamer platform options and Streamlink lookup** — override a site's own options (quality, login details, Streamlink arguments) for a single streamer, and switch a streamer, site, or template to Streamlink when a site's built-in stream lookup stops working.
- **Uploads you can watch, with a record of where files went** — a live progress badge on the streamer's card while files upload, and afterwards every file's destination, size, and result on the job page, kept across restarts.
- **Email notifications work end to end** — SMTP delivery is implemented, and the Email channel is no longer locked behind a "Coming soon" label, so you can create and edit one like any other type. Relays needing no credentials are supported.
- **A notification language per channel** — alerts sent to a Telegram chat, a webhook, or a shared mailbox can each be written in their own language, independent of the language you use in the interface.
- **Reliability pass** — monitoring recovers after a restart instead of leaving backed-off streamers unchecked, recording size and duration limits now apply to streams whose keyframes aren't recognized, HLS recordings keep data they used to drop, a failed conversion no longer leaves a truncated file at the destination, and cancelling one pipeline step now cancels its whole run.

### Review before upgrading
- **The server no longer starts with an empty signing secret.** `JWT_SECRET` has always been documented as required, but an empty value was previously accepted and used as an empty signing key. For local use only, set `AUTH_DISABLED=true` together with `API_BIND_ADDRESS=127.0.0.1` (or `::1`); any other bind address rejects that opt-out. The web interface likewise requires `SESSION_SECRET` in production. Setups created from the provided Docker Compose file already have both.
- **Importing a backup is now all-or-nothing.** An import runs as a single transaction and rolls back completely if anything is rejected. Validation is stricter, so a bundle exported by an older version that relied on missing fields being filled in for it may be refused — re-export it from this version.
- **Disabled accounts and forced password changes are enforced on every request**, not just at sign-in, so an existing token no longer keeps working until it expires.

Full release notes: https://docs.srec.rs/en/release-notes/v0.5.0 · 中文版：https://docs.srec.rs/zh/release-notes/v0.5.0
