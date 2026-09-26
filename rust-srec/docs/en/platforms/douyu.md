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

::: info
- **Authentication**: App extraction uses anonymous playback. Browser login cookies do not authenticate App playback. A valid `acf_did` supplies its device ID unless `device_id` overrides it. For rooms requiring browser authentication, select the deprecated Web method and configure cookies in **Settings** → **Platform** → **Douyu**. Access to restricted streams is not guaranteed.
- **Preferred Format**: Douyu primarily uses **FLV** for live streams.
- **Quality Control**: Use the `rate` setting to choose quality (0 for source/original).
- **CDN Switching**: You can specify a preferred CDN in the configuration if you face buffering issues.
- **Interactive Games**: You can choose to automatically skip "Interactive Games" recordings using `disable_interactive_game`.
:::

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
| `device_id` | Unset by default | Explicit 32-character alphanumeric or ASCII UUID-shaped DID; overrides `acf_did` and the source setting. |
| `device_id_mode` | `local` (default), `server`, `default` | Without an explicit DID or valid cookie, generate one locally, register with Douyu, or use the fixed compatibility DID. |
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

The App model, generated user agent, and device ID are initialized on the first App playback request, then reused for retries and CDN changes. Offline checks and Web extraction do not initialize an App device. Device options are still validated when configuring the extractor. Server registration has a five-second timeout, requires network access, and is reused by the same extractor; failures are reported without silently switching identities. To keep an identity across extractor instances or application restarts, configure `device_id` or a valid `acf_did` cookie.

The default local DID uses the Android client's timestamp-based MD5 algorithm. `device_id_mode: "default"` uses `10000000000000000000000000001511`.
