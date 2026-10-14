# TikTok

[TikTok LIVE](https://www.tiktok.com/live) is TikTok's live-streaming service.

## URL Format

```
https://www.tiktok.com/@{username}/live
```

The `@username` is the channel ID, so a streamer stays monitored after going offline.

## Features

- ✅ FLV, HLS and MPEG-DASH streams
- ✅ Multiple qualities (Original, 1080p60, 720p, 540p, 360p, audio-only) with AVC and HEVC variants
- ✅ Danmaku collection (chat, emotes, and gifts over the webcast WebSocket)
- ✅ Stream-end detection from the chat socket
- ❌ Login-only / subscriber-only rooms

## Configuration

Platform options are under **Settings** → **Platforms** → **TikTok**. **Nothing is required** for public rooms.

| Option | Default | Description |
|--------|---------|-------------|
| **Extraction API Mode** (`api_mode`) | `auto` | `auto` uses the signature-free `api-live/user/room` JSON endpoint and falls back to the live page HTML when it fails. `web` and `html` force one path. |
| **Force Origin Quality** (`force_origin_quality`) | off | Only keep the `origin` quality when TikTok offers it as FLV or HLS. When `origin` is offered only as DASH, every quality is kept, even with `dash` first in **Preferred Formats**. |

::: info
- **Authentication**: No cookies are needed. Chat collection registers a temporary `ttwid` session automatically; providing browser cookies is optional.
- **Region locks**: Some rooms are unavailable in certain regions. Use a proxy if extraction reports region-locked content (see [Proxies](../concepts/configuration.md#proxies)).
- **Danmaku**: Chat messages, emote-only messages, and gifts are captured. Combo gifts are recorded once with the final count.
- **MPEG-DASH**: Most qualities, including audio-only, are also offered as DASH, which records to MP4. FLV is preferred over DASH: it is recorded even when a quality you prefer, such as Original, is offered only as DASH, and even when `dash` is listed after another format. To record DASH, put `dash` first in **Preferred Formats** (or use it alone) in the TikTok platform settings, a template or the streamer, and use the Mesio engine (see [MPEG-DASH recording](../concepts/engines.md#mpeg-dash)). The web player cannot play DASH streams.
:::
