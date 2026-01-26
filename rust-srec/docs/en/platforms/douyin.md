# Douyin

[Douyin](https://www.douyin.com) (抖音) is China's leading short video and live streaming platform.

## URL Format

```
https://live.douyin.com/{room_id}
```

## Features

- ✅ FLV and HLS streams
- ✅ Danmaku collection
- ✅ Multiple quality options
- ✅ Choice between PC and Mobile API
- ✅ Interactive game stream detection

## Authentication

Cookies are often required for high-quality streams or to avoid rate limits. The most important cookie for Douyin is `ttwid`.

Configure in **Settings (Settings)** → **Platform (Platform)** → **Douyin (Platform-douyin)**.

::: warning
- **Stream Quality**: Use `force_origin_quality` in configuration to attempt to force the highest available quality. (Experimental: may result in no video streams if the requested quality is unavailable).
- **Region Restriction**: Some streams are region-restricted and may require a Chinese proxy/VPN.
- **Unsupported Content**: **Radio** (audio-only) streams are currently not supported.
:::

::: info
- **Double Screen**: Support for double screen stream data is enabled by default.
- **Interactive Games**: You can choose to skip "Interactive Games" (互动玩法) recordings.
- **Mobile API**: If the PC API fails, try enabling `force_mobile_api`.
:::
