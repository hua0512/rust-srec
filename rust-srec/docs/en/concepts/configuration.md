# Configuration layers {#configuration}

Use global settings for defaults, platform settings for one service, templates for groups of streamers, and streamer overrides for exceptions.

## Inheritance

Settings are applied in this order, from lowest to highest priority:

1. Global
2. Platform
3. Template, if assigned
4. Streamer

For example, if the global output format is `flv` and a template sets `mp4`, streamers using that template record as MP4. A streamer that explicitly sets `flv` uses FLV instead. Remove that override to inherit the template again.

## Create and use a template

1. Create a template containing the settings shared by a group of streamers.
2. Assign it when creating or editing each streamer.
3. Leave fields inherited unless that streamer needs a different value.
4. Edit the template to update the group. Check the rules below for recordings already in progress.

A template can also provide platform-specific overrides for pipelines and platform options (extractor settings). For streamers on that platform they take precedence over the template's own pipelines and are merged into its platform options. Other settings cannot be overridden per platform within a template.

## Important merge rules

- A supplied scalar value overrides the lower layer; an omitted value inherits it.
- A pipeline override replaces the entire pipeline, not individual steps.
- Empty quality, format, and CDN preference lists retain lower-layer preferences. CDN blacklists are combined.
- An empty cookie string can override lower-layer recording cookies. Remove the override to inherit credentials rather than entering an empty string.
- Platform options merge by key and ignore `null` overrides. Engine overrides use JSON Merge Patch, where `null` removes a key.

For supported JSON keys, credential-source selection, and examples for API clients, use the [override reference](../reference/configuration-overrides.md). Schedules and daylight-saving behavior are covered in [Recording schedules](../guides/schedules.md).

## When a change reaches a recording already in progress {#when-a-change-reaches-a-recording-already-in-progress}

Changes made through the interface or API do not require a service restart, but some settings apply only to the next download.

A running download does pick up:

- `min_segment_size_bytes` and the segment and session-complete pipeline definitions, re-read from
  the merged config as each segment completes. Clearing a pipeline in configuration does not clear
  it for a session already running — the re-read only replaces a pipeline that is set.
- `max_concurrent_downloads`, the queue freshness threshold, the GPU probe interval and pipeline
  worker concurrency, when global settings are saved.
- A new monitoring interval, which is applied to each live streamer — but only
  when `streamer_check_delay_ms`, `offline_check_delay_ms` or `offline_check_count` actually
  changed. A streamer whose check is already due runs that check first and applies the new
  interval afterwards.

It does not pick up `output_folder`, `output_filename_template`, `output_file_format`,
`max_download_duration_secs` or `max_part_size_bytes`. Those are resolved once when the download
starts and stay fixed for its whole duration, segment rotations included — a rotated segment
reuses the base name captured at the start. Editing any of them applies to the next download of
that streamer, not to the next segment of the current one.

A pipeline job that is already queued keeps the DAG definition it was created with.

<div id="the-4-layer-hierarchy" class="legacy-section">

This section is now in [Configuration layers](./configuration.md#inheritance).

</div>

<div id="what-gets-produced-mergedconfig" class="legacy-section">

This section is now in [Configuration resolution](../development/configuration.md#what-gets-produced-mergedconfig).

</div>

<div id="where-each-setting-is-configured" class="legacy-section">

This section is now in [Configuration overrides](../reference/configuration-overrides.md#where-each-setting-is-configured).

</div>

<div id="merge-rules-important-details" class="legacy-section">

This section is now in [Configuration overrides](../reference/configuration-overrides.md#merge-rules-important-details).

</div>

<div id="scalars-higher-layer-overrides" class="legacy-section">

This section is now in [Configuration overrides](../reference/configuration-overrides.md#scalars-higher-layer-overrides).

</div>

<div id="offline-detection-and-download-failure-recovery-share-one-count" class="legacy-section">

This section is now in [Configuration overrides](../reference/configuration-overrides.md#offline-detection-and-download-failure-recovery-share-one-count).

</div>

<div id="cookies-present-wins-including-empty-strings" class="legacy-section">

This section is now in [Configuration overrides](../reference/configuration-overrides.md#cookies-present-wins-including-empty-strings).

</div>

<div id="stream-selection-merged-by-streamselectionconfig-merge" class="legacy-section">

This section is now in [Configuration overrides](../reference/configuration-overrides.md#stream-selection-merged-by-streamselectionconfig-merge).

</div>

<div id="pipelines-higher-layer-replaces-the-whole-pipeline" class="legacy-section">

This section is now in [Configuration overrides](../reference/configuration-overrides.md#pipelines-higher-layer-replaces-the-whole-pipeline).

</div>

<div id="platform-extras-shallow-json-merge-null-does-not-override" class="legacy-section">

This section is now in [Configuration overrides](../reference/configuration-overrides.md#platform-extras-shallow-json-merge-null-does-not-override).

</div>

<div id="platform-extractor-options-platform-extras" class="legacy-section">

This section is now in [Configuration overrides](../reference/configuration-overrides.md#platform-extractor-options-platform-extras).

</div>

<div id="credentials-cookies-refresh-token-are-resolved-separately" class="legacy-section">

This section is now in [Configuration overrides](../reference/configuration-overrides.md#credentials-cookies-refresh-token-are-resolved-separately).

</div>

<div id="streamer-overrides-streamer-specific-config" class="legacy-section">

This section is now in [Configuration overrides](../reference/configuration-overrides.md#streamer-overrides-streamer-specific-config).

</div>

<div id="engine-and-extractor-selection" class="legacy-section">

This section is now in [Configuration overrides](../reference/configuration-overrides.md#engine-and-extractor-selection).

</div>

<div id="download-engine" class="legacy-section">

This section is now in [Configuration overrides](../reference/configuration-overrides.md#download-engine).

</div>

<div id="extractor" class="legacy-section">

This section is now in [Configuration overrides](../reference/configuration-overrides.md#extractor).

</div>

<div id="engines-override-template-only" class="legacy-section">

This section is now in [Configuration overrides](../reference/configuration-overrides.md#engines-override-template-only).

</div>

<div id="hot-reload-cache-and-update-events" class="legacy-section">

This section is now in [Configuration layers](./configuration.md#when-a-change-reaches-a-recording-already-in-progress).

</div>

<div id="filter-timezones-and-boundaries" class="legacy-section">

This section is now in [Recording schedules](../guides/schedules.md#filter-timezones-and-boundaries).

</div>
