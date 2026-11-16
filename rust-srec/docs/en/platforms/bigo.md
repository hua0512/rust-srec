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
- ✅ Live status from the room's public profile, without an account
- ✅ Danmaku collection (guest WebSocket: public chat and gifts)
- ✅ Password-protected rooms
- ❌ Multi-quality / multi-bitrate selection on the public API path
- ✅ Signed-in session from pasted cookies (for when Bigo asks for a login before handing out a stream)
- ❌ Browser-only paths that may offer higher quality (e.g. 1080p HEVC)

## Configuration

Platform options are under **Settings** → **Platforms** → **Bigo**. Public rooms usually need nothing — add a URL and record. If checks of a live room fail with a login-required error, add an [account profile](../concepts/configuration.md#account-profiles-and-selection) with cookies from a logged-in browser; see **Authentication** below.

| Option | Default | Description |
|--------|---------|-------------|
| **Stream Password** (`stream_password`) | empty | Default password for locked rooms. Override per streamer with `?pwd=...` on the URL. |

### Password rooms

1. Set `stream_password` at the platform (or streamer) level, **or**
2. Append `?pwd=yourpassword` to the streamer URL (URL wins over the platform default).

Without a password, locked rooms fail with a private-content error.

### Danmaku

When danmaku is enabled for the streamer:

- **Recorded**: public chat and gifts (guest WebSocket; no account needed).
- **Not supported**: sending chat, enter notices, hearts, follow/share notices.

::: info
- **Quality**: The public API exposes one media playlist per room (labeled `live`). That stream is usually mid-quality portrait video around **480p–540p** (for example ~480×854 H.264). There is no multi-bitrate ladder on this path. Browser-reported 1080p HEVC, when present, is not available through this API.
- **Live status**: Whether a room is live comes from its public profile, which Bigo answers without an account and is never sent one, so an account never makes a live room read as offline. A check of an offline room is one request.
- **Authentication**: Once a room is live, its stream comes from a second request, which uses the account if one is selected and the room password if one is set. Bigo may ask for a login there, for example on some networks or after many requests from the same network. Such checks fail with a login-required error instead of showing the room as offline; add an account to fix them. Copy the `bigo.tv` cookies from a browser where you are logged in; they must include `tid` and `yyuid`. Bigo logins last a short time, so accounts are refreshed every hour the way the website does it. When Bigo asks an account to log in, rust-srec checks whether the login still works. An expired login is marked as needing a new login. A working one, which Bigo sometimes turns away for a while, for example after many requests, rests for 30 minutes and is then tried again. Receiving chat does not need an account.
- **Restricted accounts**: Bigo sometimes restricts an account without signing it out, and such an account sees every room as offline. When the public profile shows a room as live but the account sees it offline, rust-srec asks both again a few seconds later. If the profile still shows the room as live and the account still sees it offline, the account is treated like one Bigo turned away: it rests for 30 minutes and is then tried again, and a pool with failover moves on to its next account.
- **Checking without an account**: While no selected account can be used, for example because it is resting or needs a new login, checks of the streamers using it stop, including whether they are live. Select Bigo accounts as a pool, even a pool with a single account, and turn on **Check without an account when no account can be used**, so live status keeps being checked and live rooms are tried without an account, on days when Bigo allows that. A **Fixed** selection cannot do this.
:::
