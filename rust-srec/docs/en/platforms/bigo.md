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

- ✅ HLS streams (single public quality, usually labeled `live`, typically ~540p portrait)
- ✅ Danmaku collection (guest WebSocket: public chat and gifts)
- ✅ Password-protected rooms
- ✅ Integrity token minting for website-parity API requests (enabled by default)
- ❌ Multi-quality / multi-bitrate selection on the public API path
- ✅ Signed-in session from pasted cookies (for when Bigo asks for a login)
- ❌ Browser-only paths that may offer higher quality (e.g. 1080p HEVC)

## Configuration

Platform options are under **Settings** → **Platforms** → **Bigo**. Public rooms usually need nothing — add a URL and record. If checks fail with a login-required error, add an [account profile](../concepts/configuration.md#account-profiles-and-selection) with cookies from a logged-in browser; see **Authentication** below.

| Option | Default | Description |
|--------|---------|-------------|
| **Stream Password** (`stream_password`) | empty | Default password for locked rooms. Override per streamer with `?pwd=...` on the URL. |
| **Mint Integrity Token** (`mint_token`) | on | Sends a website-style integrity token with studio requests. Without it, Bigo asks for a login before it reports whether a room is live, so turn it off only if minting fails on your network and an account is configured. |

### Password rooms

1. Set `stream_password` at the platform (or streamer) level, **or**
2. Append `?pwd=yourpassword` to the streamer URL (URL wins over the platform default).

Without a password, locked rooms fail with a private-content error.

### Danmaku

When danmaku is enabled for the streamer:

- **Recorded**: public chat and gifts (guest WebSocket; no account needed).
- **Not supported**: sending chat, enter notices, hearts, follow/share notices.

::: info
- **Quality**: The public API exposes one media playlist per room (labeled `live`). That stream is usually mid-quality portrait video around **480p–540p** (for example ~480×854 H.264). There is no multi-bitrate ladder on this path, and integrity tokens do not provide access to a higher-quality public stream. Browser-reported 1080p HEVC, when present, is not available through this API.
- **Authentication**: Anonymous checks rely on the integrity token, and Bigo can still ask for a login, for example after many requests from the same network. Such checks fail with a login-required error instead of showing the room as offline; add an account to fix them. Copy the `bigo.tv` cookies from a browser where you are logged in; they must include `tid` and `yyuid`. An account whose cookies Bigo no longer accepts is marked as needing a new login. Receiving chat does not need an account.
- **Network**: If integrity token minting is blocked, add an account and disable **Mint Integrity Token**.
:::
