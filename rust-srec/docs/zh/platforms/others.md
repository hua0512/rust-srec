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

账号是可选的，账号出现问题也绝不会中断监控或录制：账号无法使用时，微博主播会在不使用账号的情况下检测。凭据配置中显示的是本次登录的 weibo.com Cookie；登录的其余信息保存在 **刷新令牌** 中，无需修改。微博账号每天首次使用时以及在账号的 **…** 菜单中选择 **校验** 时会向微博检查一次。被微博退出登录的账号会被标记为无效，并发送 **凭据无效** 通知；在您点击 **重新登录** 之前，录制会在不使用该账号的情况下继续。如果每日检查被微博拦截，账号不会被标记为无效，并照常使用。

从已登录 weibo.com 的浏览器粘贴的 Cookie 同样会被检查。如果 weibo.com 认为这些 Cookie 未登录，账号同样会被标记为无效；如果 weibo.com 显示的页面中没有该账号，**校验** 会提示该账号无法检查。
