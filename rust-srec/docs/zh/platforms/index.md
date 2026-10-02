# 支持的平台

rust-srec 支持 14 个直播平台，可自动检测并录制直播流。

## 平台列表

| 平台 | URL 格式 | 协议 | 弹幕 |
|------|----------|------|------|
| [Bilibili](./bilibili.md) | `live.bilibili.com/{room_id}` | FLV/HLS | ✅ |
| [抖音](./douyin.md) | `live.douyin.com/{room_id}` | FLV/HLS | ✅ |
| [斗鱼](./douyu.md) | `douyu.com/{room_id}` | FLV | ✅ |
| [虎牙](./huya.md) | `huya.com/{room_id}` | FLV/HLS | ✅ |
| [Bigo Live](./bigo.md) | `bigo.tv/{id}` | HLS | ✅ |
| [AcFun](./others.md#acfun) | `acfun.cn/live/{room_id}` | HLS | ❌ |
| [PandaTV](./others.md#熊猫直播) | `pandalive.co.kr/play/{id}` | HLS | ❌ |
| [小红书](./others.md#小红书) | `xhslink.com/m/{id}`, `xhslink.com/o/{id}` | FLV/HLS | ❌ |
| [微博](./others.md#微博) | `weibo.com/u/{uid} or weibo.com/l/wblive/p/show/{id}` | HLS | ❌ |
| [Twitch](./twitch.md) | `twitch.tv/{channel}` | HLS | ✅ |
| [TikTok](./tiktok.md) | `tiktok.com/@{user}/live` | FLV/HLS | ✅ |
| [Twitcasting](./others.md#twitcasting) | `twitcasting.tv/{user}` | HLS | ✅ |
| [Picarto](./others.md#picarto) | `picarto.tv/{user}` | HLS/MP4 | ❌ |
| [SOOP](./soop.md) | `play.sooplive.co.kr/{channel}` | HLS | ✅ |

## 通用配置

每个平台可通过 **设置** → **平台** 进行配置。

### 认证

部分平台需要 Cookie 以获取：
- 更高画质
- 地区限制内容
- 订阅专属内容

::: tip 画质提示
如果你发现录制的画质低于预期（如只有 480p），请尝试添加已登录账号的 Cookie。许多平台会将高清画质限制在登录用户范围内。
:::

详见各平台页面。

### 直播信息查看

您可以使用内置播放器查看任何在线主播的可用直播流详情：
1. 前往 **侧边栏 (Sidebar)**。
2. 点击 **播放器 (Player)** 选项。
3. 在播放器视图中，您可以查看到所有可用的 **格式 (Formats)** (FLV, HLS)、**CDN** 以及 **画质 (Qualities)**。
4. 这可以帮助您验证当前配置（如 Cookie）是否已生效，并成功解锁更高画质或不同格式。

### 多账号与平台能力限制

通过[账号配置](../concepts/configuration.md#账号配置与选择)选择固定账号或有序的轮询/优先账号池。自动故障切换需要平台提供明确的账号认证失败或账号级限流类型。离线结果、内容限制、网络故障及范围不明的限流不能成为逐个尝试账号的理由。无法区分这些失败的平台采用保守行为；不支持验证本身不代表账号不可用。

平台准入限制由所有凭据配置、旧版请求、显式 Cookie 解析、刷新和二维码登录共享。增加账号不会增加平台请求额度。这些请求（包括匿名请求）遇到平台级或范围不明的限流时，该平台的所有请求都会暂停，时长为平台给出的重试间隔，未给出时为 60 秒。Bilibili 二维码/令牌请求及 SOOP 刷新会保留平台指定的重试间隔，即使它长于默认值。操作时限包含准入和刷新等待，修复与提取共同消耗有界尝试预算。

二维码生成时即绑定所选所有者/凭据配置。改变目标需要重新生成二维码，并发轮询不会把一次登录写给两个账号。过期或重启可能需要新二维码；已在本地完成的回执可安全重复查询。每次生成或轮询请求最长 30 秒，放弃的二维码过期后一分钟内会清除其登录数据。状态界面区分管理性禁用、凭据无效、临时冷却及不支持验证。

托管播放将 Cookie 和上游签名媒体保留在服务器，播放器使用经过授权的临时句柄。淘汰、重启、凭据/策略变更或过期均需要重新解析。刷新播放源时会先用同一账号续期；若句柄已过期或账号选择已更改，播放器会重新解析该来源，此时可能改用账号池中的其他账号。默认最多 1,024 个上下文，空闲 15 分钟、绝对存活 12 小时，因此持续播放最终也可能需要更新。显式原始 Cookie 是隔离的临时覆盖，不能与托管凭据请求混用。

托管 HLS 在服务器解析 `EXT-X-DEFINE` 定义、显式从父清单 `IMPORT` 的变量和 `QUERYPARAM` 查询变量，再重写资源 URI，不向播放器返回变量定义或签名值。每份清单最多 64 个变量，总大小不超过 64 KiB；展开后的单个值或 URI 不超过 16 KiB。格式错误、缺失、重复或循环引用的变量会明确报错。目前不支持非 URI 属性中的变量替换、内容引导（content steering）JSON 及插播广告资源列表（interstitial asset list）JSON。在 URI 属性之外指向上游媒体的标签（例如 Twitch 预取提示和厂商 URL 属性，包括带签名的相对路径）会从托管播放列表中移除。接受低延迟查询参数 `_HLS_msn`、`_HLS_part` 和 `_HLS_skip`，但不向上游转发。

Twitch 凭据配置可以只填写访问令牌而不填写 Cookie。托管选择只使用该配置的令牌，不会补用通用设置中的 `oauth_token`。旧版转换会复制实际生效的 Twitch 令牌，以及提取器实际使用的抖音 `ttwid` 或斗鱼 `device_id` 到新凭据配置。复制后的账号信息不再跟随原配置的后续修改；房间密码等非账号设置仍按正常规则继承。
