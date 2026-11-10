# Supported Platforms

rust-srec supports 14 streaming platforms with automatic stream detection and recording.

## Platform List

| Platform | URL Format | Protocol | Danmaku |
|----------|------------|----------|---------|
| [Bilibili](./bilibili.md) | `live.bilibili.com/{room_id}` | FLV/HLS | ✅ |
| [Douyin](./douyin.md) | `live.douyin.com/{room_id}` | FLV/HLS | ✅ |
| [Douyu](./douyu.md) | `douyu.com/{room_id}` | FLV | ✅ |
| [Huya](./huya.md) | `huya.com/{room_id}` | FLV/HLS | ✅ |
| [Bigo Live](./bigo.md) | `bigo.tv/{id}` | HLS | ✅ |
| [AcFun](./others.md#acfun) | `live.acfun.cn/live/{room_id}` | FLV | ❌ |
| [PandaTV](./others.md#pandatv) | `pandalive.co.kr/play/{id}` | HLS | ❌ |
| [Redbook](./others.md#redbook-小红书) | `xhslink.com/m/{id}`, `xhslink.com/o/{id}` | FLV/HLS | ❌ |
| [Weibo](./others.md#weibo) | `weibo.com/u/{uid}`, `weibo.com/l/wblive/p/show/{id}` | FLV/HLS | ❌ |
| [Twitch](./twitch.md) | `twitch.tv/{channel}` | HLS | ✅ |
| [TikTok](./tiktok.md) | `tiktok.com/@{user}/live` | FLV/HLS/DASH | ✅ |
| [Twitcasting](./others.md#twitcasting) | `twitcasting.tv/{user}` | HLS | ✅ |
| [Picarto](./others.md#picarto) | `picarto.tv/{user}` | HLS/MP4 | ❌ |
| [SOOP](./soop.md) | `play.sooplive.co.kr/{channel}`, `play.sooplive.com/{channel}` | HLS | ✅ |

## Common Configuration

Each platform can be configured at the platform level via **Settings** → **Platforms**.

### Authentication

Some platforms require a logged-in [account profile](../concepts/configuration.md#account-profiles-and-selection) for:
- Higher quality streams
- Region-restricted content
- Subscriber-only streams

::: tip Stream Quality
If you are getting lower resolution than expected (e.g., 480p instead of 1080p), try adding a logged-in account on the platform's **Network** tab and selecting it. Many platforms restrict high-definition streams to authenticated users.
:::

See individual platform pages for authentication details.

### Sites handled by Streamlink {#sites-handled-by-streamlink}

A streamer URL that no platform above recognizes, but the `streamlink` CLI can handle (for example YouTube or Kick), is added to the **Streamlink** platform. Its accounts are chosen on each streamer, not on the platform or in a template; see [Streamlink accounts](../concepts/configuration.md#streamlink-accounts).

### Batch detection {#batch-detection}

Bilibili and Twitch can report whether many streamers are live in a single request. To use this, turn on **Batch Detection** on the platform's **Specific** tab. It is off by default.

With it on, rust-srec asks the platform about up to 100 of its streamers at once, and uses the answer for the routine check of each streamer that is not recording:

- A streamer the answer shows as offline counts as checked, with no request of its own.
- A streamer the answer shows as live gets the usual full check, and recording starts as usual.
- If the batch request fails, or the answer leaves a streamer out, those streamers are checked one by one, as without the setting. After a failed batch request, the next one is tried 30 seconds later, then at growing intervals of up to 10 minutes; meanwhile the check history shows ordinary bars.

An answer is reused for up to a quarter of the check interval (**Streamer Check**, 60 seconds by default). With 250 Bilibili streamers at the default interval, that is about 12 requests a minute instead of about 250, which makes rate limits and platform anti-abuse checks less likely. With only a few streamers on a platform, most checks still make a request of their own.

What you give up:

- A stream start is usually noticed up to a quarter of the check interval later than without the setting: about 75 seconds instead of 60 at the default interval. If the batch request itself is slow to answer, the delay can be as long as that request takes (at most 30 seconds).
- The batch request is made without an account, over the connection the streamer itself uses (its own, its template's, the platform's or the global proxy setting), not an account's own [proxy setting](../concepts/configuration.md#account-proxies). Streamers on different connections are asked about in separate requests. If that connection cannot reach the platform, the batch request fails and those streamers are checked one by one, as without the setting. A streamer checked with a logged-in account still uses that account once it is live, but an expired login on that account may only be noticed when the streamer goes live, not while it is offline.

Batch detection never ends a recording. A streamer that is recording, or whose stream is being confirmed as ended, is always checked on its own, and so are streamers whose extractor is set to Streamlink. A streamer's first check after rust-srec starts, after it is added or after the setting is turned on is still an individual check, and so is every check of a streamer whose last check failed, until one succeeds.

A template's platform override or a streamer's **Platform options** tab can set it differently. The switch there shows the template's or streamer's own value, not the platform's, so to turn it off for a streamer while the platform has it on, switch it on and then off again. To make a template or streamer follow the platform again, remove `batch_detection` in that tab's **JSON** view.

Checks answered by a batch request appear in the streamer's check history as shorter, lighter bars than other offline checks, and their tooltip says so.

### Stream Inspection

You can use the built-in player to inspect available stream details for any live streamer:
1. Go to the **Sidebar**.
2. Click on the **Player** option.
3. In the player view, you can see all available **Formats** (FLV, HLS), **CDNs**, and **Qualities**.
4. Check whether your configuration, including the selected account, provides access to the expected qualities and formats.

### Multiple accounts and provider limits

Use [account profiles](../concepts/configuration.md#account-profiles-and-selection) to choose a fixed account or an ordered priority or round-robin pool. Automatic failover happens when the platform rejects an account's login, or throttles an account whose [proxy setting](../concepts/configuration.md#account-proxies) gives it a different connection from the next account. Offline results, content restrictions and network failures do not justify trying every account, and a throttle on a connection the remaining accounts share ends the check. Providers that cannot distinguish these failures remain conservative; unsupported validation does not by itself make an account unusable.

Platform admission limits apply across profiles, anonymous requests, explicit raw-cookie parses, refresh and QR login. Adding accounts does not multiply the platform's request allowance. A rate limit seen by any of these requests, including anonymous ones, pauses every request to that platform on the same connection for the platform's retry delay, or 60 seconds when it gives none. Direct, the system proxy and each saved proxy are separate [connections](../concepts/configuration.md#proxy-rate-limits), whichever setting or account chose them, and each pause sends a *Platform rate limited* notification naming its connection when it starts. A requested delay longer than 15 minutes is shortened to 15 minutes, so a single response cannot stop recording for hours. Bilibili QR/token requests, Douyu QR login and session renewal, and SOOP refresh pass the provider's retry delay through the same limit. An operation deadline includes admission and refresh waits. Repair consumes the same bounded attempt budget as extraction.

QR login targets the platform or profile chosen when the code is generated. Changing the dialog's target requires a fresh code. Concurrent polls cannot apply one login to two accounts. Expiry or restart can require generating a new code; completed local receipts are safe to poll again. Each generate or poll request is limited to 30 seconds, and an abandoned code's login data is cleared within a minute of expiry. Profile/status screens distinguish administrative disable, invalid credentials and unsupported validation.

Managed playback keeps the account's cookies and provider headers on the server. The player receives the stream URLs together with an authorized temporary handle and always plays through the server proxy, which adds the account's headers only for the stream's own host and the source page's host; playlists, segments, keys and redirects on other hosts are fetched without them, and the proxy refuses a handle for any other host. Eviction, server restart, credential/policy changes or expiry require a new parse. Refreshing a player's source first renews it with the same account; if the handle expired or its selection changed, the player parses the source again, which may pick another pool account. Defaults are 1,024 contexts, 15 minutes idle and 12 hours absolute lifetime, so even active playback can eventually need renewal. Explicit raw-cookie input remains an isolated temporary override and cannot be combined with a managed-profile request.

Managed HLS playlists are rewritten the same way as other proxied playlists. `EXT-X-DEFINE` variable substitution is not supported, and low-latency `_HLS_msn`, `_HLS_part` and `_HLS_skip` query parameters are accepted but not forwarded upstream.

Twitch profiles can contain only an access token, with no cookies. The Twitch OAuth token is the profile's access token; Douyin `ttwid` and Douyu `device_id` (as `acf_did`) are cookies in the profile, not platform options. Room passwords and other non-account settings keep their normal inheritance.
