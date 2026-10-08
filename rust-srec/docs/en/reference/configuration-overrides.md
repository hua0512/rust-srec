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

The `cookies` and `proxy_config` fields that platforms, templates and global settings carried
before [account profiles](#credential-selection-json) and [proxy routes](#proxy-route-json) are
deprecated too. The first start of this version converts what the database stored in them and
leaves them empty, and imports convert them from older backups; they will be removed in a future
version.
:::

### Cookies are not a configuration field {#cookies-present-wins-including-empty-strings}

Cookies belong to account profiles, not to platform, template or streamer configuration. Which
account a scope uses is set by its [credential selection](#credential-selection-json).

The former `cookies` field of the platform and template configuration is no longer read. A request
that still sets it fails with HTTP 422 `COOKIES_REPLACED`, so a client cannot lose an account
silently; `null` or an empty string is accepted and ignored. Add the account as a profile under
`/api/credentials` and select it instead.

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

::: tip Account fields are not platform extras
Account material lives in profiles. A configuration write that contains `cookies`,
`refresh_token`, `access_token`, `oauth_token`, `ttwid`, `device_id`, `session_cookies`,
`reauth_config`, `last_cookie_check_date` or `last_cookie_check_result` — or SOOP's `username`
and `password` — at any layer, including inside `platform_specific_config` or
`platform_extras`, is rejected with a validation error. The extractor receives these only from the
selected profile. Room passwords (`stream_password`, Bigo and TwitCasting `password`) are content
settings and stay.
:::

## Credentials come from account profiles {#credentials-cookies-refresh-token-are-resolved-separately}

Cookies, refresh and access tokens, and SOOP logins are stored in account profiles that belong to
a platform. Each scope's [credential selection](#credential-selection-json) decides which profiles
a check uses; the selected profile's material is passed to the extractor, download and chat
collection, and never appears in `MergedConfig` or serialized config APIs. See
[Account profiles and selection](../concepts/configuration.md#account-profiles-and-selection).

## Player upstream proxy {#player-upstream-proxy}

Choosing **Server proxy** makes web and desktop playback connect the same way as URL
extraction: through the source's effective [proxy setting](../concepts/configuration.md#choosing-a-proxy).
Registered streamers use their resolved setting (including template and streamer choices);
other source URLs use the platform's setting when the platform is recognized, otherwise the global
one. Playback with an account goes through the connection its extraction used, the account's own
setting included, because platforms may sign stream URLs for that address. **Direct** playback
connects from the browser and does not use the server's proxy.

The original source URL is retained through HLS playlists, segments, and keys, so a
CDN URL does not accidentally select different settings. Configuration updates apply
to subsequent requests; reload a continuous FLV/MPEG-TS stream to change its existing
connection. A proxy that cannot be used fails playback rather than falling back to
direct access. The frontend and backend must be upgraded together for web playback.

## Streamer overrides: `streamer_specific_config` {#streamer-overrides-streamer-specific-config}

`streamer_specific_config` is an untyped JSON object. Unknown keys are ignored.

Supported keys that affect `MergedConfig`:

- `output_folder`, `output_filename_template`, `output_file_format`
- `min_segment_size_bytes`, `max_download_duration_secs`, `max_part_size_bytes`
- `record_danmu`, `danmu_statistics`, `download_engine`, `extractor`,
  `offline_check_count`, `offline_check_delay_ms`
- `proxy_route` (route object; see [Proxy route JSON](#proxy-route-json))
- `stream_selection_config` (JSON object)
- `download_retry_policy` (JSON object)
- `pipeline`, `session_complete_pipeline`, `paired_segment_pipeline` (JSON objects)
- `platform_extras` (JSON object)

`credential_selection` chooses the streamer's accounts (see below). Account fields such as
`cookies` and `refresh_token` are rejected here, as on every layer.

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

## Credential selection JSON

Requests, responses and backups carry a selection as `credential_selection` in the configuration
it belongs to: on the platform configuration (as a JSON string), inside a template's
`platform_overrides[canonical_platform_name]`, or inside a streamer's `streamer_specific_config`.
Platform names are case-sensitive here; use the exact name returned by platform configuration.
Omitted update fields keep the stored policy, whereas `{ "mode": "inherit" }` explicitly resets
the scope to inherit. This holds for platform, template and streamer saves alike: a template
override or streamer document without `credential_selection` keeps the stored selection. A scope
that inherits is returned without `credential_selection`.

```json
{
  "credential_selection": {
    "mode": "pool",
    "credential_ids": ["profile-uuid-a", "profile-uuid-b"],
    "strategy": "priority",
    "failover": true,
    "max_attempts": 3
  }
}
```

Other policies are `{ "mode": "none" }`, `{ "mode": "inherit" }`, and
`{ "mode": "fixed", "credential_id": "profile-uuid-a" }`. Pools accept `priority`
or `round_robin`, require a nonempty unique ordered ID list, and accept 1–10 total attempts.
Omitted `strategy`, `failover` and `max_attempts` default to `priority`, `true` and `3`.
A one-member pool is valid. Unknown modes/fields are rejected. A selection that names a missing
profile or another platform's profile fails with HTTP 409 `CREDENTIAL_REFERENCE_INACCESSIBLE`
and lists the configuration that selects it. Disabled profiles may remain referenced, but are
unavailable at execution time. A selected profile cannot be deleted: the request fails with HTTP 409
`CREDENTIAL_PROFILE_REFERENCED` and lists the selecting configurations. A streamer whose URL moves
it to another platform loses its own selection and inherits on the new platform, even if the
request repeats the old selection; a streamer that is deleted stops selecting immediately. See
[selection and inheritance](../concepts/configuration.md#account-profiles-and-selection).

On the `streamlink` platform only a streamer stores a selection, and only `none` or `fixed`. A
selection on that platform or in a template's `platform_overrides["streamlink"]`, or a pool on a
Streamlink streamer, fails with HTTP 422 `CREDENTIAL_SELECTION_PER_STREAMER`. Backup import moves
such selections onto the Streamlink streamers instead. A Streamlink streamer without its own
selection uses the account whose `sites` cover its URL, or none. Profiles take `sites` on create
and update (omitting it on update keeps them); only Streamlink profiles accept them, an invalid
site fails with HTTP 422, and a site another profile already has fails with HTTP 409
`CREDENTIAL_SITE_TAKEN`, naming that profile. For a Streamlink streamer,
`GET /api/credentials/selection` also returns `site`: the streamer's host, the site that decides
its inherited account, and the profiles whose sites cover the host. See
[Streamlink accounts](../concepts/configuration.md#streamlink-accounts).

## Proxy route JSON {#proxy-route-json}

Each scope's proxy setting is `proxy_route`: a field of the global, platform and template
configuration, and a key inside a streamer's `streamer_specific_config`. Accounts carry it too,
when they are created or edited and when a QR login creates one.

```json
{ "proxy_route": { "kind": "proxy", "id": "proxy-uuid" } }
```

`kind` is `inherit`, `direct`, `system` or `proxy`; only `proxy` takes an `id`, the ID of a
[saved proxy](../api/index.md#proxies). Unknown kinds and extra fields are rejected. The global
setting cannot be `inherit` (HTTP 422 `PROXY_ROUTE_INVALID`). For an account, `inherit` means
following the proxy of the recording that uses it. An omitted or `null` `proxy_route` keeps the
stored setting, whereas `{ "kind": "inherit" }` resets it. A streamer that inherits is returned
without `proxy_route`. Naming a proxy that does not exist fails with HTTP 422 `PROXY_NOT_FOUND`.
See [choosing how to connect](../concepts/configuration.md#choosing-a-proxy) for precedence.

The former `proxy_config` object is no longer read. A request that still sets it — on the global,
platform or template configuration, inside `streamer_specific_config`, or inside a template's
`platform_overrides` — fails with HTTP 422 `PROXY_CONFIG_REPLACED`, so a client cannot lose its
proxy silently; `null` is accepted and ignored. Backups written before saved proxies still carry
it and are [converted on import](../operations/backup-restore.md#proxies-in-backups).
