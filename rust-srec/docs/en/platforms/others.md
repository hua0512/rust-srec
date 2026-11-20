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

### QR code login

1. Go to **Settings** → **Platforms** and open **Weibo**
2. In the **Network** tab, click **Add account**, enter a label, keep **Scan QR code** selected and click **Show QR code** (to sign an existing account in again, use **Log in again** on its row or **Log in with QR code** in its **…** menu)
3. Scan the QR code with the Weibo app and confirm the login
4. The profile is saved when the login completes; to use it, click **Change** above the account list, select it under **Account selection** and save the page

The account is optional, and a problem with it never stops monitoring or recording: whenever it cannot be used, Weibo streamers are checked without an account. The profile shows the login's weibo.com cookies; the rest of the login is kept with it in **Refresh token**, which you can leave as it is. Weibo accounts are checked with Weibo on their first use each day and when you choose **Validate** in the account's **…** menu. An account Weibo has signed out is marked invalid and you get the **Credential invalid** notification; recording goes on without it until you use **Log in again**. If Weibo blocks the daily check instead, the account is not marked invalid and is used as it is.

Cookies pasted from a browser signed in to weibo.com are checked too. If weibo.com treats them as signed out, the account is marked invalid in the same way; if weibo.com shows a page without the account, **Validate** reports that the account cannot be checked.
