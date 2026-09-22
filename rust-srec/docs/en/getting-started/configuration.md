# Basic configuration {#configuration}

Start with a working installation and [one successful recording](./first-recording.md). Set these defaults before adding more streamers.

## Choose where recordings are saved

Open **Settings → Global → Output Folder** and choose a writable recording directory. Docker uses `/app/output` inside the container; the Compose file maps it to the host's `OUTPUT_DIR`. A fresh systemd installation uses `/var/lib/rust-srec/output`. Existing installations retain the folder saved in their database.

Use `{streamer}/%Y-%m-%d` within your output layout to group recordings by streamer and date. Use `%H-%M-%S_{title}` as the filename template. See [filename placeholders](../reference/filenames.md) and [storage paths](../operations/storage.md).

## Choose an engine and recording limits

Keep Mesio for the first recording unless the platform requires another engine. Compare compatibility and features in [Recording engines](../concepts/engines.md).

Set download concurrency to a value your network and disk can sustain. Use duration or part-size limits to split long recordings. Detailed defaults and units are in the [settings reference](../reference/settings.md).

## Add platform credentials when needed

Use the relevant [platform guide](../platforms/) to determine whether login or cookies are required. Set credentials at the platform level when several streamers share an account; use a template or streamer override when they need different accounts.

## Reuse settings with templates

Create a template for shared recording settings and assign it to the relevant streamers. A streamer override takes precedence over its template. See [Configuration layers](../concepts/configuration.md) for inheritance rules and when changes take effect.

## Optional recording features

- Enable [danmu recording and statistics](../guides/danmu.md) to capture chat.
- Add [recording schedules](../guides/schedules.md) to limit recording hours.
- Create a [workflow](../concepts/pipeline.md) for conversion, thumbnails, or uploads.
- Configure [notifications](../concepts/notifications.md) for recording failures or storage alerts.

After changing the setup, verify another recording and confirm its files appear in the expected directory.

<div id="basic-configuration" class="legacy-section">

This section is now in [Make Your First Recording](./first-recording.md).

</div>

<div id="adding-your-first-streamer" class="legacy-section">

This section is now in [Make Your First Recording](./first-recording.md).

</div>

<div id="global-settings" class="legacy-section">

This section is now in [Settings reference](../reference/settings.md#global-settings).

</div>

<div id="file-configuration" class="legacy-section">

This section is now in [Settings reference](../reference/settings.md#file-configuration).

</div>

<div id="danmu-statistics" class="legacy-section">

This section is now in [Settings reference](../reference/settings.md#danmu-statistics).

</div>

<div id="resource-limits" class="legacy-section">

This section is now in [Settings reference](../reference/settings.md#resource-limits).

</div>

<div id="concurrency-performance" class="legacy-section">

This section is now in [Settings reference](../reference/settings.md#concurrency-performance).

</div>

<div id="network-system" class="legacy-section">

This section is now in [Settings reference](../reference/settings.md#network-system).

</div>

<div id="retention" class="legacy-section">

This section is now in [Settings reference](../reference/settings.md#retention).

</div>

<div id="pipeline-configuration" class="legacy-section">

This section is now in [Settings reference](../reference/settings.md#pipeline-configuration).

</div>

<div id="environment-variables" class="legacy-section">

This section is now in [Environment Variables](../reference/environment.md#environment-variables).

</div>

<div id="general" class="legacy-section">

This section is now in [Environment Variables](../reference/environment.md#general).

</div>

<div id="paths" class="legacy-section">

This section is now in [Environment Variables](../reference/environment.md#paths).

</div>

<div id="shutdown" class="legacy-section">

This section is now in [Environment Variables](../reference/environment.md#shutdown).

</div>

<div id="network" class="legacy-section">

This section is now in [Environment Variables](../reference/environment.md#network).

</div>

<div id="security-auth" class="legacy-section">

This section is now in [Environment Variables](../reference/environment.md#security-auth).

</div>

<div id="login-throttling" class="legacy-section">

This section is now in [Environment Variables](../reference/environment.md#login-throttling).

</div>

<div id="token-expiration" class="legacy-section">

This section is now in [Environment Variables](../reference/environment.md#token-expiration).

</div>

<div id="browser-notifications-web-push-vapid" class="legacy-section">

This section is now in [Environment Variables](../reference/environment.md#browser-notifications-web-push-vapid).

</div>

<div id="backend-service" class="legacy-section">

This section is now in [Environment Variables](../reference/environment.md#backend-service).

</div>

<div id="resource-limits-docker" class="legacy-section">

This section is now in [Environment Variables](../reference/environment.md#resource-limits-docker).

</div>

<div id="filename-template-variables" class="legacy-section">

This section is now in [Filename Template Variables](../reference/filenames.md#filename-template-variables).

</div>

<div id="curly-brace-variables" class="legacy-section">

This section is now in [Filename Template Variables](../reference/filenames.md#curly-brace-variables).

</div>

<div id="percent-placeholders-ffmpeg-style" class="legacy-section">

This section is now in [Filename Template Variables](../reference/filenames.md#percent-placeholders-ffmpeg-style).

</div>

<div id="pipeline-destination-placeholders" class="legacy-section">

This section is now in [Filename Template Variables](../reference/filenames.md#pipeline-destination-placeholders).

</div>
