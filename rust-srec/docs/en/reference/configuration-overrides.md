# Configuration overrides

Reference for API clients and advanced configuration. Start with [Configuration layers](../concepts/configuration.md) for examples of inheritance.

## Where each setting is configured {#where-each-setting-is-configured}

Not every field is available at every layer. The list below reflects what the resolver and
builder actually read.

- Global-only (base defaults + runtime settings): `auto_thumbnail`, concurrency/job limits,
  scheduler delays, log filter directives
- Platform-only: `fetch_delay_ms`, `download_delay_ms`, `platform_specific_config`
- Template-only: `platform_overrides`, `engines_override`
- Streamer-only: `streamer_specific_config` (JSON object; see below)

::: tip Column names differ between layers
Platform and template store `stream_selection_config` (JSON), which becomes
`MergedConfig.stream_selection`; streamer overrides use the same key `stream_selection_config`.
The global layer names its engine and extractor defaults `default_download_engine` and
`default_extractor`, while platform, template and streamer use `download_engine` and
`extractor`.
:::

## Merge rules (important details) {#merge-rules-important-details}

The builder is intentionally conservative: most fields are "override if present".

### Scalars: higher layer overrides {#scalars-higher-layer-overrides}

For most string/number/bool fields, a higher layer replaces the value when it provides one.

### Offline detection and download failure recovery share one count {#offline-detection-and-download-failure-recovery-share-one-count}

`offline_check_count` is the single inherited tolerance for consecutive offline signals. The
runtime uses the resolved per-streamer value for both:

- consecutive offline status checks before confirming that a streamer has gone offline
- consecutive download failures before placing the streamer into temporary cooldown

The default is `3`. Download failures apply a minimum threshold of `2`, even when
`offline_check_count` is set to `1`, to avoid entering cooldown after one transient CDN or
network failure. Once the threshold is reached, cooldown starts at 60 seconds and doubles after
each additional consecutive failure, up to one hour. A successful status check or sustained
download progress clears the accumulated failure state.

`offline_check_delay_ms` controls the interval between offline confirmation checks and the
related session hysteresis window. It does not control the cooldown duration.

Neither value can resolve below its floor: `offline_check_count` ends up at least `1` and
`offline_check_delay_ms` at least `1000`. The clamps run on the platform, template and streamer
layers and once more when the merged config is built, so a below-floor global value is corrected
before anything reads it.

::: warning Deprecated compatibility formats
The serialized `StreamerMetadata` aliases `effective_offline_check_count` and
`effective_offline_check_delay_ms` are deprecated. Persisted `TransientError` events that omit
`backoff_threshold` are also deprecated. These compatibility formats will be removed in a future
version. New integrations must use `offline_check_count` and `offline_check_delay_ms`, and include
`backoff_threshold` in every serialized transient-error event.
:::

### Cookies: "present wins" (including empty strings) {#cookies-present-wins-including-empty-strings}

Cookies are treated as a single optional string. If a higher layer provides `cookies`, it
overrides lower layers.

::: tip Cookies best practice
Avoid setting cookies to an empty string. An empty string is still "present" and will override
lower layers, effectively disabling fallback cookies.
:::

### Stream selection: merged by `StreamSelectionConfig::merge` {#stream-selection-merged-by-streamselectionconfig-merge}

Stream selection is merged with special semantics:

- `preferred_formats`: overrides only if `Some(non_empty_vec)`
- `preferred_media_formats`, `preferred_qualities`, `preferred_cdns`: override only if non-empty
- `min_bitrate`, `max_bitrate`: override only if non-zero
- `blacklisted_cdns`: unioned instead of replaced, so a higher layer can only add exclusions

This allows a template to specify only the parts it cares about without losing platform defaults.

### Pipelines: higher layer replaces the whole pipeline {#pipelines-higher-layer-replaces-the-whole-pipeline}

Pipelines are parsed from JSON into a `DagPipelineDefinition`. When a layer provides a pipeline,
it replaces the previous pipeline definition as a whole (there is no step-by-step merge).

A template can also carry pipelines inside `platform_overrides[platform_name]`. Those are more
specific than the template's own top-level `pipeline`, `session_complete_pipeline` and
`paired_segment_pipeline` fields, so the resolver applies them after the template layer and they
win over it.

See:

- [Workflow guide](../concepts/pipeline.md)

### Platform extras: shallow JSON merge, `null` does not override {#platform-extras-shallow-json-merge-null-does-not-override}

Platform extractor options are carried via `platform_extras` (a JSON blob) and merged with a
shallow object merge:

- If both sides are JSON objects, keys from the higher layer overwrite keys from the lower layer.
- `null` values in the higher layer are ignored (they do not override).
- If either side is not an object, the higher layer wins.

::: tip Clearing a platform_extras key
`platform_extras` uses a shallow merge and ignores `null` in the overlay. This means a higher
layer cannot "unset" a lower-layer key via `null`; it can only override with a non-null value.
:::

## Platform extractor options (`platform_extras`) {#platform-extractor-options-platform-extras}

`platform_extras` is sourced and merged from these places:

- Platform layer: `platform_config.platform_specific_config`
- Template layer: `template_config.platform_overrides[platform_name]`
- Streamer layer: `streamers.streamer_specific_config.platform_extras`

The same merge function is applied each time in layer order.

::: tip About credentials in platform extras
Platform, template and streamer records may all contain credential-related keys. Each layer is
stripped of `refresh_token`, `access_token`, `session_cookies`, `last_cookie_check_date` and
`last_cookie_check_result` before it is merged into `platform_extras`, so extractor config never
carries credentials.
:::

## Credentials (`cookies` + `refresh_token`) are resolved separately {#credentials-cookies-refresh-token-are-resolved-separately}

The runtime derives a `credential_source` (a sidecar on `ResolvedStreamerContext`) for
authentication and refresh-token handling. It is intentionally not part of `MergedConfig` and
must not be exposed via serialized config APIs.

Precedence (highest to lowest):

1. Streamer override: `streamer_specific_config.cookies`
   (+ optional `streamer_specific_config.refresh_token` / `access_token`)
2. Template: `template_config.cookies`
   (+ optional `template_config.platform_overrides[platform].refresh_token` / `access_token`)
3. Platform: `platform_config.cookies`
   (+ optional `platform_config.platform_specific_config.refresh_token` / `access_token`)

Unlike `MergedConfig.cookies`, an empty or whitespace-only `cookies` value does **not** claim the
credential source: that layer is skipped and the next one down is considered. A `refresh_token`
or `access_token` is only picked up from the layer whose cookies won, so a `refresh_token` on a
streamer with no streamer-level cookies is ignored.

A platform can also produce a credential source without cookies: for SOOP, a
`platform_specific_config` carrying `username` and `password` yields a credential source whose
cookies are minted on first use.

## Player upstream proxy

Choosing **Server proxy** makes web and desktop playback use the same effective
`proxy_config` as URL extraction. Registered streamers use their merged configuration
(including template and streamer overrides); other source URLs use the platform
override when recognized, otherwise the global configuration. **Direct** playback
connects from the browser and does not use the server's upstream proxy.

The original source URL is retained through HLS playlists, segments, and keys, so a
CDN URL does not accidentally select different settings. Configuration updates apply
to subsequent requests; reload a continuous FLV/MPEG-TS stream to change its existing
connection. Invalid explicit proxy URLs fail playback rather than falling back to
direct access. The frontend and backend must be upgraded together for web playback.

## Streamer overrides: `streamer_specific_config` {#streamer-overrides-streamer-specific-config}

`streamer_specific_config` is an untyped JSON object. Unknown keys are ignored.

Supported keys that affect `MergedConfig`:

- `output_folder`, `output_filename_template`, `output_file_format`
- `min_segment_size_bytes`, `max_download_duration_secs`, `max_part_size_bytes`
- `record_danmu`, `danmu_statistics`, `cookies`, `download_engine`, `extractor`,
  `offline_check_count`, `offline_check_delay_ms`
- `proxy_config` (JSON object)
- `stream_selection_config` (JSON object)
- `download_retry_policy` (JSON object)
- `pipeline`, `session_complete_pipeline`, `paired_segment_pipeline` (JSON objects)
- `platform_extras` (JSON object)

Keys used by the credentials subsystem (not part of `MergedConfig`):

- `refresh_token`, `access_token`

Both are read from the same layer whose `cookies` won. They are two of the five keys —
`refresh_token`, `access_token`, `session_cookies`, `last_cookie_check_date` and
`last_cookie_check_result` — stripped from every layer before it is merged into
`platform_extras`.

::: tip Invalid JSON is ignored
Most JSON fields in platform/template/global records are parsed best-effort. If JSON parsing
fails, the resolver logs a warning and falls back to defaults or the previous layer. The same
applies inside `streamer_specific_config`: a key whose value has the wrong shape is skipped and
the lower layer is inherited, rather than failing the whole resolve.

`platform_extras` is the exception. Its value is taken as-is rather than shape-checked, and the
merge falls back to "overlay wins" whenever either side is not an object — so a scalar or an
array there replaces the extras accumulated from the lower layers instead of being skipped.
:::

## Engine and extractor selection {#engine-and-extractor-selection}

### `download_engine` {#download-engine}

`download_engine` is a string that selects which download engine configuration to use. It can be:

- A built-in engine type string (`ffmpeg`, `streamlink`, `mesio`)
- A custom engine configuration ID stored in the `engine_configuration` table

An ID that matches neither falls back to the manager's default engine.

### `extractor` {#extractor}

`extractor` selects which extractor resolves the stream URL. It is independent of
`download_engine`, which only decides how the resolved URL is pulled. Valid values:

- `auto` (the default): dispatch on the URL regex registry
- `streamlink`: resolve through Streamlink

A layer that stores `NULL` or an empty string expresses no preference and inherits from the layer
below. An unrecognized name is logged and ignored the same way, so a typo degrades to inheritance
instead of failing the resolve.

### `engines_override` (template-only) {#engines-override-template-only}

Templates can provide `engines_override`, a JSON object of:

- `engine_id` -> `override_value`

When a download starts, the Download Manager checks whether there is an override entry for the
selected engine ID. If so, it:

1. Loads the base engine config (default config for built-in types, DB config for custom IDs)
2. Applies the override with JSON Merge Patch semantics: nested objects are merged key by key,
   and a `null` in the override removes that key
3. Creates a dedicated engine instance for that override, keyed by a hash of the override so its
   circuit-breaker state stays separate from the un-overridden engine

::: tip `null` means something different here
`engines_override` removes a key when the override sets it to `null`, whereas `platform_extras`
ignores `null` in the overlay.
:::
