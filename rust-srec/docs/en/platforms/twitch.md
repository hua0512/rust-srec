# Twitch

[Twitch](https://www.twitch.tv) is the world's leading live streaming platform for gamers.

## URL Format

```
https://www.twitch.tv/{channel_name}
```

## Features

- ✅ HLS streams
- ✅ Danmaku collection (via IRC WebSocket)
- ✅ Multiple quality options
- ✅ Subscriber-only stream support (requires OAuth)

::: info
- **Authentication**: Most streams are public. For **subscriber-only** streams, you must provide an `oauth_token` in the configuration.
- **OAuth Token**: You can obtain your token from the browser's cookies or by using specialized Twitch token tools. (Format: `oauth:xxxxxxxxxxxxxx`).
- **Danmaku**: Chat messages and "Bits" (cheers) are captured as danmaku.
- **Proxy**: If you encounter buffering or region blocks, consider using a proxy (see [Docker Configuration](../getting-started/docker.md#proxy-configuration)).
:::
