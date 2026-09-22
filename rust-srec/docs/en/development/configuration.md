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
- Network: `proxy_config`, `cookies`
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
