# 其他平台

## AcFun

- **URL**: `https://live.acfun.cn/live/{房间号}`
- **协议**: FLV
- **弹幕**: ❌ 不支持

## 熊猫直播

- **URL**: `https://www.pandalive.co.kr/play/{id}`
- **协议**: HLS
- **弹幕**: ❌ 不支持

## Picarto

- **URL**: `https://picarto.tv/{频道}`
- **协议**: HLS/MP4
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

::: info 带密码的直播
在 Twitcasting 平台设置中填写 **直播密码**，或把密码以 `?password=...` 的形式加到主播 URL 中（例如 `https://twitcasting.tv/{用户}?password=secret`）。URL 中的密码优先。
:::

## 微博

- **URL**: `https://weibo.com/u/{uid}`, `https://weibo.com/{uid}`, `https://m.weibo.cn/u/{uid}`, `https://m.weibo.cn/profile/{uid}`, `https://weibo.com/l/wblive/p/show/{id}`
- **协议**: FLV/HLS
- **弹幕**: ❌ 不支持
::: info
用户主页链接无需账号：rust-srec 会像手机网页版对待访客那样，以游客身份查询该用户。不支持用自定义名称代替数字用户 ID 的链接（例如 `https://weibo.com/{name}`），请打开主页后复制带数字 ID 的链接。

[账号凭据配置](../concepts/configuration.md#账号配置与选择)是可选的，只在微博不接受游客查询时使用。此时如果微博提示该账号已退出登录，账号会被标记为需要重新登录，并发送账号通知。
:::

### 二维码登录

1. 进入 **设置** → **平台**，打开 **微博**
2. 在 **网络** 标签页点击 **添加账号**，填写名称，保持选中 **扫码登录** 并点击 **显示二维码**（已有账号需要重新登录时，点击其所在行的 **重新登录**，或在 **…** 菜单中选择 **扫码登录**）
3. 使用微博 App 扫码并确认登录
4. 登录完成后凭据配置即保存；要使用它，点击账号列表上方的 **更改**，在 **账号选择** 中选中它并保存页面

凭据配置保存 weibo.com 的 Cookie 用于请求，并把登录的其余部分（微博其他站点的登录信息）保存为刷新令牌。微博账号每天首次使用时以及在账号的 **…** 菜单中选择 **校验** 时会向微博检查一次。被微博退出登录的账号会被标记为无效，点击 **重新登录** 即可恢复。如果检查被微博拦截，账号不会被标记为无效，稍后会再次检查。

从已登录 weibo.com 的浏览器粘贴的 Cookie 同样可以检查，前提是 weibo.com 能显示这些 Cookie 属于哪个账号；否则 **校验** 会提示该账号无法检查。
