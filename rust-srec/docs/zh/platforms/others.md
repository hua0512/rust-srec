# 其他平台

## AcFun

- **URL**: `https://live.acfun.cn/live/{房间号}`
- **协议**: HLS
- **弹幕**: ❌ 不支持

## 熊猫直播

- **URL**: `https://www.pandalive.co.kr/{房间号}`
- **协议**: FLV
- **弹幕**: ❌ 不支持

## Picarto

- **URL**: `https://picarto.tv/{频道}`
- **协议**: HLS
- **弹幕**: ❌ 不支持

## 小红书

- **URL**: `https://xhslink.com/m/{id}`, `https://xhslink.com/o/{id}`
- **协议**: FLV/HLS
- **弹幕**: ❌ 不支持

::: info App 分享链接
请使用 `xhslink.com` 分享链接。`xiaohongshu.com/user/profile/...` 这类个人主页直链目前不支持。

注意：分享链接每次开播都会变化，请在每次开播时重新复制分享链接。

分享链接无需账号。如果选用了[账号凭据配置](../concepts/configuration.md#账号配置与选择)，其 Cookie 必须包含非空的 `a1` 字段，否则会报解析错误。
:::

## Twitcasting

- **URL**: `https://twitcasting.tv/{用户}`
- **协议**: HLS
- **弹幕**: ✅ 已支持

## 微博

- **URL**: `https://weibo.com/l/wblive/p/show/{id}`, `https://weibo.com/u/{uid}`
- **协议**: HLS
- **弹幕**: ❌ 不支持
::: info
使用 `https://weibo.com/u/{uid}` 格式的链接需要已登录的[账号凭据配置](../concepts/configuration.md#账号配置与选择)。
:::
