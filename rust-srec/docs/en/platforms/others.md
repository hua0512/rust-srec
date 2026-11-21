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
Set **Stream Password** in the Twitcasting platform settings, or add the password to the streamer URL as `?password=...` (for example `https://twitcasting.tv/{user}?password=secret`). A password in the URL takes priority.
:::

## Weibo

- **URL**: `https://weibo.com/u/{uid}`, `https://weibo.com/{uid}`, `https://m.weibo.cn/u/{uid}`, `https://m.weibo.cn/profile/{uid}`, `https://weibo.com/l/wblive/p/show/{id}`
- **Protocol**: FLV/HLS
- **Danmaku**: ❌ Not supported
::: info
User links work without an account: rust-srec looks the user up as a guest, the way the mobile site does for a visitor. Links with a custom name in place of the numeric user ID, such as `https://weibo.com/{name}`, are not supported; open the profile and copy the link with the number.

An [account profile](../concepts/configuration.md#account-profiles-and-selection) is optional. It is used only when Weibo does not accept the guest lookup. If Weibo then reports that the account is signed out, the account is marked as needing a new login and you get the account notification.
:::
