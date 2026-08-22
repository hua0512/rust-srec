# SOOP

[SOOP](https://www.sooplive.co.kr), formerly AfreecaTV, is a Korean live streaming platform.

## URL Format

```
https://play.sooplive.co.kr/{channel_id}
https://play.sooplive.com/{channel_id}
https://play.afreecatv.com/{channel_id}
```

Broadcast URLs that include a broadcast number are also supported:

```
https://play.sooplive.com/{channel_id}/{broadcast_number}
```

## Features

- ✅ HLS streams with multiple qualities (`original`, `hd4000`, `hd`, `sd`, …)
- ✅ Login-required broadcast support (account or cookies)
- ✅ Password-protected room support
- ✅ Danmaku collection (guest WebSocket: public chat and gifts)
- ❌ Sending chat (guest receive only)

## Configuration

Platform options are under **Settings** → **Platforms** → **SOOP**. Public rooms need no config.

| Option | Default | Description |
|--------|---------|-------------|
| **Username** / **Password** | empty | SOOP account for login-required (e.g. 19+) broadcasts. Prefer cookies for permanently restricted channels. |
| **Stream Password** (`stream_password`) | empty | Default password for locked rooms. Override per streamer with `?pwd=...` on the URL. |

### Password rooms

1. Set `stream_password` at the platform (or streamer) level, **or**
2. Append `?pwd=yourpassword` to the streamer URL (URL wins over the platform default).

Without a password, locked rooms fail with a private-content error.

### Danmaku

When danmaku is enabled for the streamer:

- **Recorded**: public chat and gifts (balloons, chocolate, superchat, subscription, …) on open rooms; guest WebSocket, no account needed.
- **Not supported**: sending chat, full enter-notice taxonomy.

Login-gated adult rooms need cookies (or account credentials for video) before chat metadata is available.

::: info
- **Quality**: Each public quality is resolved lazily with a short-lived AID key just before download.
- **Authentication**: Configure username/password for login-required streams. Session cookies are validated via SOOP private info, re-minted when invalid, and saved to the platform/template/streamer cookie field so later polls reuse the session. You can also paste cookies manually.

- **Network**: Outside supported regions SOOP often returns a GDPR geo stub (`RESULT=0` with `GDPR=true`) instead of live metadata. Use a Korean network or proxy; rust-srec reports this as a region error rather than “offline”.
- **Recording**: Placeholder “preloading” segments in SOOP playlists are skipped so they are not written into the file.
:::
