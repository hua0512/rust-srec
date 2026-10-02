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

Some platforms require cookies for:
- Higher quality streams
- Region-restricted content
- Subscriber-only streams

::: tip Stream Quality
If you are getting lower resolution than expected (e.g., 480p instead of 1080p), try adding cookies from a logged-in account. Many platforms restrict high-definition streams to authenticated users.
:::

See individual platform pages for authentication details.

### Stream Inspection

You can use the built-in player to inspect available stream details for any live streamer:
1. Go to the **Sidebar**.
2. Click on the **Player** option.
3. In the player view, you can see all available **Formats** (FLV, HLS), **CDNs**, and **Qualities**.
4. Check whether your configuration, including cookies, provides access to the expected qualities and formats.

### Multiple accounts and provider limits

Use [account profiles](../concepts/configuration.md#account-profiles-and-selection) to choose a fixed account or an ordered round-robin/priority pool. Automatic failover requires a typed account authentication failure or account-scoped throttle from the provider. Offline results, content restrictions, network failures and unknown throttles do not justify trying every account. Providers that cannot distinguish these failures remain conservative; unsupported validation does not by itself make an account unusable.

Platform admission limits apply across profiles, legacy requests, explicit raw-cookie parses, refresh and QR login. Adding accounts does not multiply the platform's request allowance. A platform-wide or unclassified rate limit seen by any of these requests, including anonymous ones, pauses every request to that platform for the platform's retry delay, or 60 seconds when it gives none. Bilibili QR/token requests and SOOP refresh preserve the provider's retry delay, including delays longer than the default. An operation deadline includes admission and refresh waits. Repair consumes the same bounded attempt budget as extraction.

QR login targets the owner/profile chosen when the code is generated. Changing the dialog's target requires a fresh code. Concurrent polls cannot apply one login to two accounts. Expiry or restart can require generating a new code; completed local receipts are safe to poll again. Each generate or poll request is limited to 30 seconds, and an abandoned code's login data is cleared within a minute of expiry. Profile/status screens distinguish administrative disable, invalid credentials, temporary cooldown and unsupported validation.

Managed playback keeps cookies and signed upstream media server-side. The player uses authorized temporary handles; eviction, server restart, credential/policy changes or expiry require a new parse. Refreshing a player's source first renews it with the same account; if the handle expired or its selection changed, the player parses the source again, which may pick another pool account. Defaults are 1,024 contexts, 15 minutes idle and 12 hours absolute lifetime, so even active playback can eventually need renewal. Explicit raw-cookie input remains an isolated temporary override and cannot be combined with a managed-profile request.

Managed HLS resolves `EXT-X-DEFINE` values, explicit parent `IMPORT` variables and `QUERYPARAM` variables on the server before rewriting resource URIs. Definitions and signed values are not returned to the player. Each manifest supports at most 64 variables totaling 64 KiB; an expanded value or URI is limited to 16 KiB. Malformed, missing, duplicate or cyclic variables fail explicitly. Variable substitution in non-URI attributes, content-steering JSON and interstitial asset-list JSON are not supported. Tags that name upstream media outside a URI attribute, such as Twitch prefetch hints and vendor URL attributes (including relative signed paths), are removed from managed playlists. Low-latency `_HLS_msn`, `_HLS_part` and `_HLS_skip` query parameters are accepted but are not forwarded upstream.

Twitch profiles can contain only an access token, with no cookies. Managed selection uses that profile's token and does not supplement it with a generic `oauth_token`. Legacy conversion copies the effective Twitch token and, when used by the extractor, Douyin `ttwid` or Douyu `device_id` into the new profile. These copied account values stop following later edits to the original configuration; room passwords and other non-account settings retain their normal inheritance.
