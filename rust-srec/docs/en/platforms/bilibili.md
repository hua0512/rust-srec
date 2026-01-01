# Bilibili

[Bilibili](https://www.bilibili.com) is China's leading video and live streaming platform.

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

### QR Code Login (Recommended)

1. Go to **Settings (Settings)** → **Platform (Platform)**
2. Select **Bilibili (Platform-bilibili)** → **Network (Network)** tab
3. Click **Scan Login (Qr login)**
4. Scan the QR code with Bilibili mobile app
5. Credentials are automatically saved

### Manual Cookies

Set cookies in **Platform Config** → **Bilibili**:

| Cookie | Required | Description |
|--------|----------|-------------|
| `SESSDATA` | Yes | Session token |
| `refresh_token` | Yes | Token for refreshing cookies (can be found in Browser LocalStorage as `ac_time_value`) |
| `bili_jct` | Optional | CSRF token |
| `DedeUserID` | Optional | User ID |

## Quality Options

| Quality | Description |
|---------|-------------|
| `10000` | 原画 (Original) |
| `400` | 蓝光 (Blu-ray) |
| `250` | 超清 (Super HD) |
| `150` | 高清 (HD) |
| `80` | 流畅 (Smooth) |

## Notes

- **Cookies are required for recording Super HD (1080P) and above quality.**
- Some streams require login for higher quality
- VIP-only streams require corresponding membership
