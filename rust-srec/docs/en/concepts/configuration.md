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

A template can also provide platform-specific overrides for pipelines, credential selection, and platform options (extractor settings). For streamers on that platform they take precedence over the template's own pipelines and are merged into its platform options. Other settings cannot be overridden per platform within a template.

## Important merge rules

- A supplied scalar value overrides the lower layer; an omitted value inherits it.
- A pipeline override replaces the entire pipeline, not individual steps.
- Empty quality, format, and CDN preference lists retain lower-layer preferences. CDN blacklists are combined.
- Platform options merge by key and ignore `null` overrides. Engine overrides use JSON Merge Patch, where `null` removes a key.

For supported JSON keys, account selection, and examples for API clients, use the [override reference](../reference/configuration-overrides.md). Schedules and daylight-saving behavior are covered in [Recording schedules](../guides/schedules.md).

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

## Account profiles and selection

A profile stores one account's complete cookies and supported refresh or login material for one platform. Save accounts separately; never concatenate cookies from different accounts. Profiles belong to their platform and are managed on the platform page. The platform, a template's override for that platform, and any streamer on it can each select from the platform's profiles; a selection cannot name another platform's profile. Cookies, tokens and account logins are not stored in configuration itself.

| Selection | Behavior |
| --- | --- |
| Inherit (or not set) | Follows the next layer; at the platform, it means no account. |
| None | Stops inheritance and uses no stored account credentials or automatic re-login. |
| Fixed | Uses exactly one profile; refresh and re-login target that account. |
| Pool: priority (default) | Starts with the first eligible profile in saved order; later profiles are backups. |
| Pool: round robin | Distributes independent checks across eligible profiles in saved order. Most useful when the accounts [connect through different proxies](#account-proxies). |

Resolution walks the streamer, the template override for the exact platform name, then the platform, and uses the first selection other than inherit. A pool replaces the inherited pool; lists are never combined. Global configuration does not store accounts. Pools have optional failover and a total attempt limit from 1 to 10; disabled and invalid accounts are skipped. Priority is the recommended pool: one main account with automatic backups draws less attention from platform anti-abuse checks than spreading every check across all accounts. Fixed selection never switches accounts. Unavailable credentials do not silently fall back to anonymous access.

A recording keeps its selected account through extraction, download startup, and chat collection. Ordinary polls cannot rotate a recording already running. If recovery switches the recording to another account, chat collection reconnects with that account. Account/material changes require fresh extraction for the next attempt; changing a policy or disabling a profile does not by itself stop an engine already running. Use the existing stop/disable action to stop immediately. Credential exhaustion backs off without counting as a generic streamer error or entering temporary disable. A recording that is waiting for credentials restarts as soon as an account is edited, re-enabled or logged in again, or its selection changes, without waiting for the next check.

When no account can be used, the account status says why: every usable account needs a new login, or every selected account is disabled.

A streamer whose last check found no usable account shows **Account needs login** or **Accounts disabled** on its card instead of *Offline*, with a link to its platform's accounts; a streamer that is recording keeps showing *Live*. The badge clears at the next check that reaches an account. The top bar also counts enabled accounts that need a new login, or whose automatic refresh has failed three times in a row after the platform rejected them, and lists them by platform. Disabled accounts are not counted. An expired login is flagged only where the platform reports it; elsewhere the account keeps being used and neither warning appears for it.

Profile saves happen immediately; selecting the saved profile in an unsaved configuration form still requires saving that form. Supplying new material replaces the whole bundle and clears any token it leaves out; an edit that supplies no material keeps the stored secrets. Ordinary profile views show presence/status indicators rather than secret values. Each row of the account list sums up who uses the account and when, or why it needs attention. Expanding a row names the platform, templates and streamers that select the account and the streamers recording with it, with links to their pages, and shows when the account was last used (updated at most every few minutes), checked and refreshed, any consecutive refresh failures and their reason, and, for accounts the platform renews automatically such as Douyu's, when the next renewal is due. Deleting an account that is still selected or recording lists the same configurations and recordings instead. A streamer's own selection ends when the streamer is deleted, and when an edited URL moves it to another platform it inherits there; select its accounts again on the new platform if it needs its own.

### Streamlink accounts {#streamlink-accounts}

Streamers on sites without a built-in extractor, such as YouTube or Kick, all belong to the Streamlink platform. Because that one platform covers many unrelated sites, its accounts are chosen per streamer:

- Add profiles on the Streamlink platform page, then choose **No authentication** or one **Fixed account** in each streamer's settings. A streamer that inherits uses no account.
- The Streamlink platform and template overrides for it cannot select accounts, and pools are not available, so there is no automatic failover to another account.
- Streamlink receives the profile's cookies and connects as the account's [proxy setting](#account-proxies) chooses. The account is not checked or refreshed automatically, and a login failure does not mark it invalid.
- Credentials written into the Streamlink engine's or extractor's `extra_args` are passed to Streamlink as written. They are not managed by profiles, are never checked, and apply to every streamer using those settings.

A Twitch streamer whose [extractor](../reference/configuration-overrides.md#extractor) is set to Streamlink stays on the Twitch platform and uses Twitch accounts as usual; its profile's cookies and access token are passed to Streamlink.

### Upgrading from configuration cookies {#upgrading-from-configuration-cookies}

Earlier versions stored cookies, tokens and logins (Twitch OAuth token, Douyin TTWID, Douyu device ID, SOOP username and password) directly in platform, template and streamer configuration. The first start after upgrading converts them:

- Each platform, template or streamer that carried its own credentials gets a profile on its platform, labeled after it with "(migrated)", and a fixed selection of that profile. The profile holds what its recordings used before, including the platform's SOOP login.
- Identical credentials on one platform share a single profile.
- A template's cookie is converted only for platforms whose streamers use the template.
- A cookie on the Streamlink platform, or a template cookie used by Streamlink streamers, becomes a fixed selection on each of those streamers that has no account of its own, because [Streamlink accounts](#streamlink-accounts) are chosen per streamer. Backups are imported the same way, including a Streamlink platform or template selection; an imported pool there keeps only its first account.
- Empty cookie fields are treated as unset: that scope now inherits.
- Credentials that are not valid account material are dropped, with a warning in the log.

Take a backup before upgrading; going back to an earlier version needs it. See [backup and rollback](../operations/backup-restore.md#credential-profiles-and-rollback).

## Proxies {#proxies}

Save the proxies you use under **Settings → Proxies**, then choose how each scope and account connects: directly, through the server's system proxy, or through a saved proxy.

### Saved proxies {#saved-proxies}

A saved proxy has a name and an address: `http`, `https`, `socks5` or `socks5h`, written as `scheme://host:port` without a path, with an optional username and password in their own fields. An address and username identify the exit a platform sees, so each pair can be saved only once; saving it again offers the existing proxy instead. Names are unique, ignoring case.

The list shows each proxy's address, username and how many settings use it; expand a row to see which. Passwords are never shown: when editing, leave the password empty to keep it. **Test** requests a platform's home page, or another address, once through the proxy and reports whether it answered, with the HTTP status and the time taken; the editor can test a proxy before it is saved. Any answer except the proxy rejecting its login counts as reachable. Testing an address on a private network needs **Allow private stream proxy targets**. The page also shows the system proxy detected when the server started.

A proxy cannot be deleted while a setting or account uses it; the delete dialog lists them. A template being removed releases its proxy once the recordings using it finish.

Changes are saved at once and apply to the next check, extraction or connection; work already running keeps its connection. Changing a proxy's address or username counts as changing the credentials of the accounts that choose it: they are checked again, and recordings using them switch at their next fresh extraction. Renaming a proxy or changing only its password does not.

### Choosing how to connect {#choosing-a-proxy}

Global settings choose the **Default proxy** under **Network & System**: **Direct**, **System proxy** or a saved proxy. Platforms, templates and streamers choose on their **Proxy** tab, which also offers **Inherit**. The most specific choice other than Inherit wins: streamer, then template, then platform, then global. A template that inherits leaves each streamer on its own platform's setting.

The chosen connection carries live checks and parsing, downloads with every engine, danmu, and playback through the **Server proxy**.

- **Direct** uses no proxy at all. FFmpeg and Streamlink are started without the `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY` and `NO_PROXY` environment variables, so they connect directly too.
- **System proxy** uses those [environment variables](../reference/environment.md#network) of the server. Danmu uses the first of `HTTPS_PROXY`, `ALL_PROXY` and `HTTP_PROXY` that was set when the server started, and connects directly to hosts listed in `NO_PROXY`. With none of them set, System proxy connects directly. In the desktop app, the operating system's proxy setting reaches only some requests made inside the app, never FFmpeg, Streamlink or danmu; save the proxy instead to cover every connection.
- A **saved proxy** carries all of them. FFmpeg supports only `http` proxies: an FFmpeg recording through an `https` or SOCKS proxy fails with an error saying so, and the setting warns about it. Use an `http` proxy there, or the Mesio or Streamlink engine.

A setting that names a proxy which no longer exists makes its requests fail rather than connect directly.

Notification channels, browser push, and upload tools such as rclone and BaiduPCS-Go do not use these settings; they follow the server's environment variables.

### Account proxy settings {#account-proxies}

Each account has its own proxy setting, chosen when adding or editing it: **Follow the recording's proxy** (the default), **Direct**, **System proxy** or a saved proxy. An account's own choice wins over the streamer, template and platform for everything done with the account: account checks, refreshes, renewals and QR login, live checks and parsing, downloads, danmu and playback. An account that follows the recording connects the way whatever uses it does; its own checks, refreshes, renewals and QR login then follow the platform's setting, or the global one. In the account list, an icon marks accounts with their own setting.

Changing an account's proxy setting counts as changing its credentials: the account is checked again, and recordings using it switch at their next fresh extraction.

### Rate limits per connection {#proxy-rate-limits}

Platforms throttle a connection rather than an account. Direct, System proxy and each saved proxy are separate connections; without proxy environment variables, System proxy counts as Direct. When a platform limits requests, only its requests on that connection pause, whichever setting or account chose it. A pool with failover then moves on to an account on another connection, skipping accounts that use the throttled one, and tries accounts whose connection is still paused last. Each pause sends a *Platform rate limited* notification naming the connection. Changing a saved proxy's address or username ends its pause.

### Upgrading from proxy settings {#upgrading-proxy-settings}

Earlier versions stored a proxy address and login in global, platform, template and streamer settings. The first start after upgrading converts them:

- Each distinct address and username becomes a saved proxy named after its `host:port`, with ` 2`, ` 3`… added when the name is taken, and every setting that used it chooses it. A login written into the address moves into the username and password fields, and an address without a scheme gets `http://`. Settings with the same address and username share one proxy; when their passwords differ, the first one found is kept.
- **Use System Proxy** becomes **System proxy**. On a platform, template or streamer, a proxy that was turned off becomes **Direct**, and no proxy setting becomes **Inherit**.
- An address no engine can use, such as `socks4`, becomes **Direct**, with a warning in the log.
- A global proxy that was turned off or empty becomes **System proxy** when `HTTP_PROXY`, `HTTPS_PROXY` or `ALL_PROXY` is set at that start, otherwise **Direct**. That keeps FFmpeg and Streamlink downloads on the environment's proxy, which they used before; live checks, Mesio downloads and danmu now use it too. Choose **Direct** afterwards if they should not. A new installation chooses its default the same way on its first start.

Because **Direct** now keeps FFmpeg and Streamlink off the environment's proxy, a platform, template or streamer whose proxy was turned off no longer sends their downloads through it. Choose **System proxy** there if it should.

Take a backup before upgrading; going back to an earlier version needs it. Older backups are converted the same way when imported, except for the environment rule; see [proxies in backups](../operations/backup-restore.md#proxies-in-backups).
