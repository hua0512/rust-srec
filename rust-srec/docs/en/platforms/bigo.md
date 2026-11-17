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

## Live notifications {#live-notifications}

Bigo has no request that reports many rooms at once. Instead, rust-srec can listen for BIGO's own live notifications, the alerts the BIGO app shows when a followed streamer starts streaming. This is the recommended way to monitor Bigo streamers:

- When a streamer goes live, BIGO notifies rust-srec and the streamer is checked at once. BIGO's public profile can take a moment to show the new stream, so the streamer is checked again a few times over the next minute.
- Routine checks of offline streamers are answered without a request of their own. A full check still runs at least every 10 minutes, in case BIGO misses a notification, and the first check after each connection to BIGO is a full one.
- Stream ends are not notified. They are noticed by the usual checks, as without notifications.

### Requirements

- **A signed-in BIGO account**, added on the Bigo platform's **Network** tab like any other [account profile](../concepts/configuration.md#account-profiles-and-selection). Use a dedicated account.
- rust-srec makes that account **follow every Bigo streamer you monitor** and turns on their live alerts. Streamers can see who follows them. rust-srec never unfollows: streamers you remove, turning notifications off and choosing another account all leave the follows in place.
- Turn on live notifications in that account's settings in the BIGO app.
- BIGO keeps one app session per account: opening the BIGO app with the same account disconnects rust-srec, and the other way round. Recordings never sign in to BIGO's app service with this account, so they do not disconnect it. They do use its cookies if it is also chosen for recordings on the **Network** tab.

### Turning them on

1. Open **Settings** → **Platforms** → **Bigo** → **Specific**, section **Status Checks**.
2. Turn on **Batch Detection**. For Bigo, this switch turns on live notifications.
3. Choose the account under **Notification Account**, then save.

The same section shows whether rust-srec is connected, how many streamers BIGO has confirmed, and the last error. It also counts streams that checks found started without a notification; if that count grows, turn on live notifications in the account's BIGO settings. Each Bigo streamer card shows how the streamer is monitored: **Notifications**, or **Polling · inconsistent** with the reason in its tooltip. A newly added streamer starts on polling: its BIGO account is learned by its first check, and BIGO then has to confirm the follow.

While rust-srec is not connected to BIGO, every Bigo streamer is polled as without notifications. rust-srec reconnects after 2 seconds, waiting longer after each failure, up to 30 seconds. If BIGO reports the account as signed out, rust-srec refreshes its login. If the login has expired, the account is marked as needing a new login, like any account that needs attention, and notifications wait until it is signed in again. If BIGO still reports the account as signed out after a refresh, or refuses its login, for example from this IP address, notifications are tried again every 15 minutes. Any change to the account or its proxy tries again at once.

A template's platform override or a streamer's **Platform options** tab can turn **Batch Detection** off: those streamers are then polled and not followed. Turning it on there has no effect while it is off for the platform.

### Polling

Without notifications, each Bigo streamer is checked at every check interval. Polling is inconsistent: a stream start is noticed up to one check interval late, so short streams can be missed.

## Recording {#recording}

By default, rust-srec records the broadcaster's original stream: the one the BIGO app plays, from BIGO's app servers. It needs no account and keeps the quality the broadcaster sends, often 720p or 1080p in HEVC (H.265) and sometimes H.264, where the website offers a smaller H.264 stream of about 540p. These recordings are FLV files made by the Mesio engine: a configured Mesio engine is used with its settings, and any other engine is replaced by the default Mesio engine for them. If the connection drops, rust-srec reconnects up to three times and the recording continues, in the same file unless BIGO's servers switch the stream to another resolution or codec. A recording ends when the room ends, or when the stream stops sending anything after it ran for a while; the next check then tells whether the streamer is still live. Not every browser can play HEVC, so the web interface's player may not play these recordings.

The website's HLS stream is recorded instead:

- when a stream password is set (the platform's default applies to every streamer);
- for rooms with several hosts, whose combined picture only the website has;
- when BIGO's app servers could not be reached from your network in the last 5 minutes, or the broadcaster's original stream failed to record during the current broadcast (for up to 30 minutes);
- when **Record the original stream** is turned off.

**Preferred Formats** with `hls` first tries the website's stream first, and the original stream when the website's fails. The website's stream can need an account (see **Authentication** below): a login request for it fails the check without trying the original stream.

::: warning FFmpeg version for HEVC recordings
Remuxing, converting or otherwise post-processing HEVC FLV files needs FFmpeg 6.1 or newer. The Docker image ships a current build; for a binary or desktop installation, make sure the `ffmpeg` it uses is recent enough.
:::

## Features

- ✅ [Original stream](#recording) from BIGO's app servers, without an account (HEVC or H.264, as the broadcaster sends it)
- ✅ The website's HLS stream as the fallback (one quality, typically ~540p portrait)
- ✅ [Live notifications](#live-notifications) through a signed-in account (recommended)
- ✅ Live status from the room's public profile, without an account
- ✅ Danmaku collection (guest WebSocket: public chat and gifts)
- ✅ Password-protected rooms (through the website's stream)
- ❌ Choosing a quality: each stream comes in one quality
- ❌ Separate recordings of each host in a room with several hosts
- ✅ Signed-in session from pasted cookies (for when Bigo asks for a login before handing out the website's stream)

## Configuration

Platform options are under **Settings** → **Platforms** → **Bigo**. Public rooms usually need nothing — add a URL and record — but set up [live notifications](#live-notifications) to notice stream starts within seconds. If checks of a live room fail with a login-required error, the website's stream was needed: add an [account profile](../concepts/configuration.md#account-profiles-and-selection) with cookies from a logged-in browser; see **Authentication** below.

| Option | Default | Description |
|--------|---------|-------------|
| **Record the original stream** (`native_stream`) | on | Records the broadcaster's [original stream](#recording), with the website's stream as the fallback. Turned off, only the website's stream is recorded. |
| **Stream Password** (`stream_password`) | empty | Default password for locked rooms. Override per streamer with `?pwd=...` on the URL. Set on the platform, it applies to every Bigo streamer, which then records only the website's stream. |
| **Batch Detection** (`batch_detection`) | off | Turns on [live notifications](#live-notifications). A template or streamer can only turn it off. |
| **Notification Account** (`notification_account`) | none | The signed-in account that receives live notifications. Set on the platform only. |

### Password rooms

1. Set `stream_password` at the platform (or streamer) level, **or**
2. Append `?pwd=yourpassword` to the streamer URL (URL wins over the platform default).

Without a password, locked rooms fail with a private-content error.

### Danmaku

When danmaku is enabled for the streamer:

- **Recorded**: public chat and gifts (guest WebSocket; no account needed).
- **Not supported**: sending chat, enter notices, hearts, follow/share notices.

::: info
- **Quality**: The original stream's resolution and codec (HEVC or H.264) are the broadcaster's; BIGO's servers pick which version a connection gets, so they can differ between recordings, and a change during a recording starts a new file. The website's stream is one media playlist per room, usually mid-quality portrait video around **480p–540p** (for example ~480×854 H.264). Both are labeled `live`; choose between them with **Preferred Formats** (`flv` for the original stream, `hls` for the website's).
- **Live status**: Whether a room is live comes from its public profile, which Bigo answers without an account and is never sent one, so an account never makes a live room read as offline. A check of an offline room is one request.
- **Authentication**: The original stream never uses an account. The website's stream comes from a second request, made only when that stream is tried, which uses the account if one is selected and the room password if one is set. Bigo may ask for a login there, for example on some networks or after many requests from the same network. Such checks fail with a login-required error instead of showing the room as offline; add an account to fix them. Copy the `bigo.tv` cookies from a browser where you are logged in; they must include `tid` and `yyuid`. Bigo logins last a short time, so accounts are refreshed every hour the way the website does it. When Bigo asks an account to log in, rust-srec checks whether the login still works. An expired login is marked as needing a new login. A working one, which Bigo sometimes turns away for a while, for example after many requests, rests for 30 minutes and is then tried again. Receiving chat does not need an account.
- **Restricted accounts**: Bigo sometimes restricts an account without signing it out, and such an account sees every room as offline. When the public profile shows a room as live but the account sees it offline, rust-srec looks the room up on BIGO's app servers without an account. If the room has ended, it reads as offline. Otherwise the profile and the account are asked again a few seconds later, as they can disagree for a moment when a stream starts or ends. If the profile still shows the room as live and the account still sees it offline, the account is treated like one Bigo turned away: it rests for 30 minutes and is then tried again, and a pool with failover moves on to its next account.
- **Checking without an account**: While no selected account can be used, for example because it is resting or needs a new login, checks of the streamers using it stop, including whether they are live. Select Bigo accounts as a pool, even a pool with a single account, and turn on **Check without an account when no account can be used**, so live status keeps being checked and live rooms are tried without an account, on days when Bigo allows that. A **Fixed** selection cannot do this.
:::
