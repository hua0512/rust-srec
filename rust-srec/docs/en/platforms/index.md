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
| [AcFun](./others.md#acfun) | `acfun.cn/live/{room_id}` | HLS | ❌ |
| [PandaTV](./others.md#pandatv) | `pandalive.co.kr/play/{id}` | HLS | ❌ |
| [Redbook](./others.md#redbook-小红书) | `xhslink.com/m/{id}`, `xhslink.com/o/{id}` | FLV/HLS | ❌ |
| [Weibo](./others.md#weibo) | `weibo.com/u/{uid}` | HLS | ❌ |
| [Twitch](./twitch.md) | `twitch.tv/{channel}` | HLS | ✅ |
| [TikTok](./tiktok.md) | `tiktok.com/@{user}/live` | FLV/HLS | ✅ |
| [Twitcasting](./others.md#twitcasting) | `twitcasting.tv/{user}` | HLS | ✅ |
| [Picarto](./others.md#picarto) | `picarto.tv/{user}` | HLS/MP4 | ❌ |
| [SOOP](./soop.md) | `play.sooplive.co.kr/{channel}` | HLS | ✅ |

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
