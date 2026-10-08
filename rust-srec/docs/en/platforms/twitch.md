# Twitch

[Twitch](https://www.twitch.tv) is a live streaming platform for gaming and other content.

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
- **Authentication**: Most streams are public. For **subscriber-only** streams, add a Twitch account profile with your OAuth token as its access token (**Settings** → **Platforms** → **Twitch**, **Network** tab).
- **OAuth Token**: While signed in to twitch.tv, copy the value of the `auth-token` browser cookie. Paste it as the profile's access token, or include the whole `auth-token=…` cookie in the profile's cookies. An `oauth:` prefix is accepted and ignored. The token does not expire on its own; it stops working when you sign out of that browser session or change your password.
- **Account check**: Twitch accounts are checked with Twitch on their first use each day and when you choose **Validate** in the account's **…** menu. A token Twitch rejects marks the account invalid, and recording moves on to the next account in a pool. Sign in again and replace the token to fix it; Twitch tokens cannot be refreshed automatically.
- **Streamlink extractor**: When a Twitch streamer's extractor is set to Streamlink, the selected profile's access token is passed to Streamlink's Twitch plugin as its API authorization header, along with the profile's cookies.
- **Danmaku**: Chat messages and "Bits" (cheers) are captured as danmaku.
- **Proxy**: If you encounter buffering or region blocks, consider using a proxy (see [Proxies](../concepts/configuration.md#proxies)).
:::
