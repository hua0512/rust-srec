# Douyu

[Douyu](https://www.douyu.com) (斗鱼) is a major Chinese game streaming platform.

## URL Format

```
https://www.douyu.com/{room_id}
```

## Features

- ✅ FLV streams
- ✅ Danmaku collection
- ✅ Multiple quality options (via `rate` selection)
- ✅ CDN selection support
- ✅ Interactive game stream detection
- ✅ Android App extraction with AVC / HEVC preference
- ✅ QR code login with automatic renewal

::: info
- **Authentication**: Without a signed-in account, playback is anonymous, and Douyu may hold rooms with several qualities below the original (for example at 超清). A signed-in account, added as described in [Authentication](#authentication), is sent with stream requests by both the App and the Web method and can unlock the original quality; see [Recording without an account](#recording-without-an-account). A valid `acf_did` cookie in the selected account profile supplies the device ID. Access to restricted streams is not guaranteed.
- **Preferred Format**: Douyu primarily uses **FLV** for live streams.
- **Quality Control**: Use the `rate` setting to choose quality (0 for source/original).
- **CDN Switching**: You can specify a preferred CDN in the configuration if you face buffering issues.
- **Interactive Games**: You can choose to automatically skip "Interactive Games" recordings using `disable_interactive_game`.
:::

## Authentication

Douyu accounts are [account profiles](../concepts/configuration.md#account-profiles-and-selection) on the Douyu platform.

### Recording without an account

Douyu limits what signed-out viewers get, and the two extraction methods are limited differently:

| Method | Without an account | With a signed-in account |
| --- | --- | --- |
| **App** (default) | In rooms with several qualities, usually limited to 超清, but recordings run without interruption | Original quality |
| **Web** | Can get a higher quality than App, though not always the original, but Douyu cuts the stream about every five minutes, so recordings split into many short files with brief gaps | Original quality, without the five-minute cuts |

To record in the best quality, add a signed-in account. Without one, keep the App method unless a higher quality matters more to you than uninterrupted recordings.

### QR code login

1. Go to **Settings** → **Platforms** and open **Douyu**
2. In the **Network** tab, click **Add account**, enter a label, keep **Scan QR code** selected and click **Show QR code** (to sign an existing account in again, use **Log in again** on its row or **Log in with QR code** in its **…** menu)
3. Scan the QR code with the Douyu app and confirm the login
4. The profile is saved when the login completes; to record with it, click **Change** above the account list, select it under **Account selection** and save the page

The login gives the profile a new device ID, which App playback then uses as its `acf_did`. It also saves Douyu's long-lived sign-in (`LTP0`) as the profile's refresh token. Recordings that use the profile send its session with their stream requests, which can unlock qualities Douyu keeps for signed-in viewers.

A Douyu session lasts about six days. Once the profile's session is four days old, it is renewed from the saved sign-in before the account is next used; **Refresh** in the account's **…** menu renews it at once. If a renewal fails, the account keeps its current session, a *Credential refresh failed* notification is sent, and renewal is tried again when the account is next used, an hour later at the earliest. When renewals keep failing, for example after signing out of Douyu everywhere, use **Log in again** on the account.

### Manual cookies

Add a profile and paste the cookies of a signed-in browser session. Copy all of the browser's cookies for www.douyu.com, not just a few: if some are missing, the App method records as a signed-out viewer, at lower quality. To have them renewed like a QR login, the cookies must include `dy_did`, and the `LTP0` cookie from `passport.douyu.com` must be in **Refresh token** or among the cookies. Without both, the cookies are used as they are until Douyu stops accepting them, and the profile offers no **Refresh**. A profile holding only an `acf_did` cookie supplies just the App device ID.

## Extraction settings

Set these options in the Douyu platform settings, or override them for a template or streamer. An unset option inherits the preceding configuration layer.

| Option | Values / default | Behavior |
| --- | --- | --- |
| `api_mode` | `app` (default), `web` | App uses the Android playback API. Web preserves the legacy extractor and is deprecated. |
| `rate` | Non-negative integer, default `0` | Requested quality: `0` original, `3` ultra HD, `2` HD, `1` SD. Availability depends on the room. |
| `cdn` | App default `hw`; Web default `ws-h5` | App examples: `hw`, `tct`, `hs`, `ws`. Existing `-h5` suffixes are removed for App requests. |
| `codec` | `avc` (default), `hevc` | Prefer H.264/AVC or H.265/HEVC. HEVC falls back to AVC if unavailable. |
| `device_name` | Random model by default | Android model, e.g. `OnePlus 12`. Unset generates a model such as `ABC-DE12`. |
| `os_version` | `"14"` (default) | Android version used with the model to generate the App user agent. |
| `device_id_mode` | `local` (default), `server`, `default` | Without a valid `acf_did` cookie, generate one locally, register with Douyu, or use the fixed compatibility DID. |
| `only_audio` | `false` (default) | Requests AAC without video through the legacy Web method, regardless of `api_mode`. Codec preference is ignored. |
| `request_retries` | Default `3`, minimum effective value `1` | Maximum attempts for room metadata and App playback. |
| `disable_interactive_game` | `false` (default) | Treat interactive game rooms as offline. |

Example platform-specific JSON:

```json
{
  "api_mode": "app",
  "rate": 0,
  "cdn": "hw",
  "codec": "hevc"
}
```

Douyu may dispatch a different CDN or downgrade the requested quality. Resolved streams report the returned rate/CDN and actual codec. The App method does not automatically switch to Web on playback errors; choose `web` explicitly when needed. Audio-only is the compatibility exception described above.

The App model, generated user agent, and device ID are initialized on the first App playback request, then reused for retries and CDN changes. Offline checks and Web extraction do not initialize an App device. Device options are still validated when configuring the extractor. Server registration has a five-second timeout, requires network access, and is reused by the same extractor; failures are reported without silently switching identities. To keep an identity across extractor instances or application restarts, put a valid `acf_did` cookie in the account profile.

The default local DID uses the Android client's timestamp-based MD5 algorithm. `device_id_mode: "default"` uses `10000000000000000000000000001511`.
