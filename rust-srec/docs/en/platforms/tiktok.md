# TikTok

[TikTok LIVE](https://www.tiktok.com/live) is TikTok's live-streaming service.

## URL Format

```
https://www.tiktok.com/@{username}/live
```

The `@username` is the channel ID, so a streamer stays monitored after going offline.

## Features

- ✅ FLV and HLS streams
- ✅ Multiple qualities (Original, 1080p60, 720p, 540p, 360p, audio-only) with AVC and HEVC variants
- ✅ Danmaku collection (chat, emotes, and gifts over the webcast WebSocket)
- ✅ Stream-end detection from the chat socket
- ❌ Login-only / subscriber-only rooms

## Configuration

Platform options are under **Settings** → **Platforms** → **TikTok**. **Nothing is required** for public rooms.

| Option | Default | Description |
|--------|---------|-------------|
| **Extraction API Mode** (`api_mode`) | `auto` | `auto` uses the signature-free `api-live/user/room` JSON endpoint and falls back to the live page HTML when it fails. `web` and `html` force one path. |
| **Force Origin Quality** (`force_origin_quality`) | off | Only keep the `origin` quality when TikTok offers it. |

::: info
- **Authentication**: No cookies are needed. Chat collection registers a temporary `ttwid` session automatically; providing browser cookies is optional.
- **Region locks**: Some rooms are unavailable in certain regions. Use a proxy if extraction reports region-locked content (see [Docker Configuration](../getting-started/docker.md#proxy)).
- **Danmaku**: Chat messages, emote-only messages, and gifts are captured. Combo gifts are recorded once with the final count.
:::
