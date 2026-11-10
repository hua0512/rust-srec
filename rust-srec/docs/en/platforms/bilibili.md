# Bilibili

[Bilibili](https://www.bilibili.com) is a Chinese video and live streaming platform.

## URL Format

```
https://live.bilibili.com/{room_id}
```

## Features

- ✅ FLV and HLS streams
- ✅ Danmaku collection
- ✅ Multiple quality options
- ✅ QR code login support

## Authentication

Bilibili accounts are [account profiles](../concepts/configuration.md#account-profiles-and-selection) on the Bilibili platform.

### QR Code Login (Recommended)

1. Go to **Settings** → **Platforms** and open **Bilibili**
2. In the **Network** tab, click **Add account**, enter a label, keep **Scan QR code** selected and click **Show QR code** (to log an existing account in again, use **Log in again** on its row or **Log in with QR code** in its **…** menu)
3. Scan the QR code with the Bilibili mobile app
4. The profile is saved when the login completes; to record with it, click **Change** above the account list, select it under **Account selection** and save the page

### Manual Cookies

Add a profile in the same place, choose **Paste cookies** and paste the cookies of a logged-in browser session:

| Cookie | Required | Description |
|--------|----------|-------------|
| `SESSDATA` | Yes | Session token |
| `DedeUserID` | Optional | User ID; lets danmaku connect as the account |

Pasted cookies are not renewed automatically: once Bilibili stops accepting them, the account needs a new login. Automatic refresh needs the access token and refresh token that a QR code login saves with the profile; the browser's `ac_time_value` is not such a token.

## Quality Options

Set **Preferred Quality (QN)** (`quality`) in the Bilibili platform settings, or override it for a template or streamer. It is the highest quality to request; the default is `30000`, which asks for the best quality the room offers.

| Quality | Description |
|---------|-------------|
| `30000` | 杜比视界 (Dolby Vision), default |
| `20000` | 4K |
| `10000` | 原画 (Original) |
| `401` | 蓝光杜比 (Blu-ray Dolby) |
| `400` | 蓝光 (Blu-ray) |
| `250` | 超清 (Super HD) |
| `150` | 高清 (HD) |
| `80` | 流畅 (Smooth) |
| `0` | Lowest available |

## Batch Detection

Turn on **Batch Detection** on the platform's **Specific** tab to check up to 100 offline rooms in one request; see [Batch detection](./index.md#batch-detection) for what it changes.

## Notes

::: warning
A logged-in account is required for recording Super HD (1080P) and above quality.
:::

::: info
- Some streams require login for higher quality
- VIP-only streams require corresponding membership
- HLS streams may be delayed when the streamer first goes online.
:::
