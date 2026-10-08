# Douyin

[Douyin](https://www.douyin.com) (抖音) is a Chinese short video and live streaming platform.

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

Douyin usually records without an account: when no `ttwid` cookie is supplied, one is obtained automatically. To use a logged-in session or your own `ttwid`, add its cookies as an [account profile](../concepts/configuration.md#account-profiles-and-selection) in **Settings** → **Platforms** → **Douyin**, on the **Network** tab, and select it.

::: warning
- **Stream Quality**: Use `force_origin_quality` in configuration to attempt to force the highest available quality. (Experimental: may result in no video streams if the requested quality is unavailable).
- **Region Restriction**: Some streams are region-restricted and may require a proxy in mainland China (see [Proxies](../concepts/configuration.md#proxies)) or a VPN.
- **Unsupported Content**: **Radio** (audio-only) streams are currently not supported.
:::

::: info
- **Double Screen**: Support for double screen stream data is enabled by default.
- **Interactive Games**: You can choose to skip "Interactive Games" (互动玩法) recordings.
- **Mobile API**: If the PC API fails, try enabling `force_mobile_api`.
:::
