# Bigo Live

[Bigo Live](https://www.bigo.tv) is a global live-streaming platform for mobile and desktop creators.

## URL Format

```
https://www.bigo.tv/{site_id}
https://www.bigo.tv/{locale}/{site_id}
```

Examples:

- `https://www.bigo.tv/221338632`
- `https://www.bigo.tv/ja/221338632`
- Vanity slugs such as `https://www.bigo.tv/username`

## Features

- ✅ HLS streams (single public quality, usually labeled `live`)
- ✅ Danmaku collection (guest WebSocket: public chat and gifts)
- ✅ Password-protected rooms
- ✅ Integrity token minting for website-parity API requests (enabled by default)
- ❌ Multi-quality / multi-bitrate selection on the public API path
- ❌ Account login for higher quality or sending chat

## Configuration

Platform options are under **Settings** → **Platforms** → **Bigo**. **Nothing is required** for public rooms — add a URL and record.

| Option | Default | Description |
|--------|---------|-------------|
| **Stream Password** (`stream_password`) | empty | Default password for locked rooms. Override per streamer with `?pwd=...` on the URL. |
| **Mint Integrity Token** (`mint_token`) | on | Sends a website-style integrity token with studio requests. Turn off only if minting fails on your network; most open rooms still work without it. |

### Password rooms

1. Set `stream_password` at the platform (or streamer) level, **or**
2. Append `?pwd=yourpassword` to the streamer URL (URL wins over the platform default).

Without a password, locked rooms fail with a private-content error.

### Danmaku

When danmaku is enabled for the streamer:

- **Recorded**: public chat and gifts (guest WebSocket; no account needed).
- **Not supported**: sending chat, enter notices, hearts, follow/share notices.

::: info
- **Quality**: The public API exposes one media playlist per room (typically labeled `live`). There is no multi-bitrate ladder on this path.
- **Authentication**: Cookies and OAuth are not required for recording or receiving chat.
- **Network**: If integrity token minting is blocked, disable **Mint Integrity Token** and retry.
:::
