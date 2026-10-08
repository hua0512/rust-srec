# Configuration resolution

`MergedConfigBuilder` applies each configuration layer. `ConfigResolver` loads database records and builds the effective configuration; `ConfigService` caches the result and broadcasts updates to runtime services.

The user-facing inheritance rules are in [Configuration layers](../concepts/configuration.md); JSON keys are in the [override reference](../reference/configuration-overrides.md).

## What gets produced: `MergedConfig` {#what-gets-produced-mergedconfig}

`MergedConfig` is the resolved configuration the runtime uses for monitoring, downloads, danmu,
and pipelines.

Key fields (grouped by concern):

- Output: `output_folder`, `output_filename_template`, `output_file_format`
- Limits: `min_segment_size_bytes`, `max_download_duration_secs`, `max_part_size_bytes`
- Danmu: `record_danmu`, `danmu_statistics`
- Network: `proxy_route` (a resolved route; see [Proxy routes](#proxy-routes)), `credential_policy`
- Engine: `download_engine`, `extractor`, `download_retry_policy`, `engines_override`
- Stream selection: `stream_selection`
- Pipelines: `pipeline`, `session_complete_pipeline`, `paired_segment_pipeline`
- Platform extractor options: `platform_extras`
- Timing: `fetch_delay_ms`, `download_delay_ms`, `offline_check_count`,
  `offline_check_delay_ms`
- Session UX: `auto_thumbnail`

Some runtime settings are global-only (not part of `MergedConfig`), such as concurrency
limits and log filter directives.

## Hot reload, cache, and update events {#hot-reload-cache-and-update-events}

`ConfigService` caches resolved streamer configs in memory:

- TTL: 1 hour (default)
- Concurrent request deduplication: only one in-flight resolve per streamer
- Hard resolve timeout: 30 seconds (prevents stuck in-flight entries)

When configs change via API/UI, the service invalidates relevant cache entries and broadcasts a
`ConfigUpdateEvent` so the scheduler and managers can react.

Typical invalidation patterns:

- `GlobalUpdated`: invalidate all streamers
- `PlatformUpdated`: invalidate streamers on that platform
- `TemplateUpdated`: invalidate streamers using that template
- `StreamerMetadataUpdated`: invalidate that streamer
- `EngineUpdated`: invalidate all streamers (engine usage is not tracked)
- Saved-proxy edits and deletions: invalidate all streamers and publish `GlobalUpdated`

::: tip Prefer templates
Put shared settings in a template rather than repeating them per streamer. Changing one template
then re-resolves every streamer assigned to it, instead of requiring an edit per streamer.
:::

At startup and after global, platform or template changes, resolved offline-check
settings refresh for up to 16 independent streamers at a time. The runtime event
handler still finishes each selected batch before processing its next event.
A failed lookup retains that streamer's previous metadata settings and does not
stop healthy neighbors from refreshing; streamers already marked for retirement
remain excluded from the bulk snapshot. This changes refresh scheduling, not
configuration precedence or persistent recovery acknowledgements.

## Filter snapshots

Monitor checks share immutable filter snapshots, including empty results. Up to 1,024
streamers are cached and at most 16 repository loads run concurrently; concurrent checks
for one streamer share one load. A load times out after 10 seconds. Filter order and invalid-row
skipping remain unchanged, and cancellation or failure releases waiting checks.

Successful filter edits invalidate the shared snapshot before scheduling a recheck. Imports,
streamer deletion and global/lag reconciliation invalidate affected snapshots too. A check that
already holds a snapshot finishes with that version; a retired load cannot publish over an
invalidation. External SQL or writers outside the shared runtime may remain unseen for the
30-second cache TTL; the first subsequent check refreshes expired data.

## Managed credential persistence and execution

Policies contain profile IDs, never copied secrets. Selections are rows of `credential_selections`, one per platform, per template and platform, or per streamer, with their ordered profiles in `credential_selection_members`; the API and backups carry them as `credential_selection` inside the platform, template override or streamer configuration, and the repository splits them out on write and puts them back on read. A scope without a row defers to the next layer (no account at the platform), so `inherit` is never stored; `none` terminates authentication inheritance. Profiles belong to a platform, and SQLite keeps every member on its selection's platform and refuses to delete a selected profile. It also removes the selections of a deleted platform, template or streamer, of a streamer marked deleted, and of a streamer that moves to another platform. Member counts per mode, retiring profiles and the owner's platform are checked when a selection is written. A selection lives outside the configuration row, so a streamer write that changes only its selection invalidates that streamer's merged configuration explicitly. On the Streamlink platform a streamer without its own selection resolves to the profile whose site in `credential_profile_sites` covers its URL; that layer is never stored, and a change of a profile's sites republishes the platform's configuration. Configuration never stores account material: writes carrying cookies, tokens or account logins are rejected. Cache resolved policy; obtain current secret material at execution time.

Databases from before profiles are converted once at startup: `run_migrations` runs the conversion after the SQL migrations while the `legacy_credential_upgrade_pending` marker table exists, and drops the marker in the same transaction. The migration that drops the old `cookies` columns keeps their values in `legacy_cookies`, which the conversion reads and drops with the marker. Imports of older backups apply the same conversion to the bundle before validation.

Credential material revisions protect refresh, health and login writes from stale completion. Label edits have a separate optimistic version. Policy/profile/import mutations publish through owned post-commit work; HTTP request cancellation cannot undo publication after commit. Import keys profiles by platform name, preserves free UUIDs and rejects UUID collisions with another platform's profile without label/secret matching. Imported profiles are written before the configuration that selects them, in the same SQLite transaction.

Recording bindings persist identity, material revision, policy generation and epoch without secrets. Changed accounts/revisions require a new complete extraction bundle; old URLs must never receive current cookies. Active-session references block physical deletion. Replace imports persist retirement intent, settle work outside the transaction and reap profiles only after references disappear. Configuration backups exclude runtime bindings, managed profile health, selection cursors, QR sessions and playback contexts.

Shared platform admission covers anonymous, raw and managed extraction, refresh and QR operations. Remote retry delays are capped at 15 minutes. A logical deadline includes admission, lock and repair waits, with no nested double charge. Admission backoff is keyed by platform and `RouteKey`: direct, system, or one saved proxy, whatever scope or account chose it; the system key collapses to direct when no environment proxy was detected. Failover follows an account authentication failure, or a throttle when another candidate uses a different connection; the token-bucket rate stays per platform. Coordination is process-local: separate backends sharing SQLite are not guaranteed fair rotation or one provider refresh. SQLite revision checks prevent stale writes, not distributed execution.

## Proxy routes {#proxy-routes}

Saved proxies are rows of `proxies`: a name unique without regard to case, a canonical
`scheme://host[:port]` URL, and an optional username and password stored as a pair. A unique
index on the URL and username makes one row one exit. Global, platform, template, streamer and
account rows store a `proxy_route` (`inherit`, `direct`, `system` or `proxy`) and a `proxy_id`
that references `proxies` with `ON DELETE RESTRICT`; the global route cannot inherit. A trigger
resets the route of a streamer marked deleted, so it stops holding a proxy. The API carries a
streamer's route inside `streamer_specific_config`, and the repository splits it out on write
and puts it back on read, as with credential selections. Configuration writes change a route
only when the request gives one, so internal writes never overwrite it.

Resolution takes the first non-inheriting layer (streamer, template, platform, global) and
produces a `ResolvedRoute`: the `ProxyTarget` (direct, system or an explicit endpoint) every HTTP client and engine uses, a
`RouteKey` for admission, and the deciding source for the UI. An account's own route replaces
the operation's in its credential snapshot, which extraction, download, danmu and managed
playback then share; management calls for the account resolve its own route, then the
platform's, then the global one. A route naming a missing proxy is an error, never a direct
connection. A system route reads the environment snapshot taken at startup, which danmu and
the admission key use; in-process clients defer to their own environment and operating-system
lookup.

Engines receive the `ProxyTarget`. For `Direct`, FFmpeg and Streamlink commands have
`HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY` and `NO_PROXY` (both cases) removed from their
environment, and the Streamlink extractor does the same. FFmpeg refuses any explicit proxy
other than `http` before starting.

`ProxyService` owns proxy writes. Changing a proxy's URL or username starts a new revision of
every account pinned to it, drops their health and clears the proxy's admission backoff;
renaming it or changing only the password does not. Resolved snapshots keep the route they were
resolved with.

The `legacy_proxy_upgrade_pending` marker installed by the migration makes `run_migrations` convert
the old proxy settings and streamer keys once, after the credential conversion, in one
transaction that also drops the marker. A later migration drops the old `proxy_config` columns
and keeps their values in `legacy_proxy_settings`, which the conversion reads and drops. Backup
bundles name proxies instead of carrying IDs; importing an older bundle runs the same conversion
on the bundle without the environment rule.
