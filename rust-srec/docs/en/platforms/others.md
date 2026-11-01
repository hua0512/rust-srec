# Other Platforms

## AcFun

- **URL**: `https://live.acfun.cn/live/{room_id}`
- **Protocol**: FLV
- **Danmaku**: ❌ Not supported

## PandaTV

- **URL**: `https://www.pandalive.co.kr/play/{id}`
- **Protocol**: HLS
- **Danmaku**: ❌ Not supported

## Picarto

- **URL**: `https://picarto.tv/{channel}`
- **Protocol**: HLS/MP4
- **Danmaku**: ❌ Not supported

## Redbook (小红书)

- **URL**: `https://xhslink.com/m/{id}`, `https://xhslink.com/o/{id}`
- **Protocol**: FLV/HLS
- **Danmaku**: ❌ Not supported

::: info App share links
Use `xhslink.com` share links. Direct profile URLs like `xiaohongshu.com/user/profile/...` are not supported.

Note: the share link changes every stream. Copy a fresh share link when the streamer goes live.

Share links need no account. If you select an [account profile](../concepts/configuration.md#account-profiles-and-selection), its cookies must include a non-empty `a1` value; incomplete cookies cause an extraction error.
:::

## Twitcasting

- **URL**: `https://twitcasting.tv/{user}`
- **Protocol**: HLS
- **Danmaku**: ✅ Supported

::: info Password-protected broadcasts
Add the password to the streamer URL as `?password=...`, for example `https://twitcasting.tv/{user}?password=secret`.
:::

## Weibo

- **URL**: `https://weibo.com/l/wblive/p/show/{id}`, `https://weibo.com/u/{uid}`
- **Protocol**: FLV/HLS
- **Danmaku**: ❌ Not supported
::: info
Without an account, Weibo is accessed with a built-in guest session, which can stop working. If links, especially `https://weibo.com/u/{uid}`, stop resolving, add a logged-in [account profile](../concepts/configuration.md#account-profiles-and-selection).
:::
