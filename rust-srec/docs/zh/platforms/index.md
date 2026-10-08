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

部分平台需要已登录的[账号凭据配置](../concepts/configuration.md#账号配置与选择)才能获取：
- 更高画质
- 地区限制内容
- 订阅专属内容

::: tip 画质提示
如果你发现录制的画质低于预期（如只有 480p），请尝试在该平台的 **网络** 标签页中添加已登录的账号并选中它。许多平台会将高清画质限制在登录用户范围内。
:::

详见各平台页面。

### 由 Streamlink 处理的网站 {#sites-handled-by-streamlink}

上表平台都无法识别、但 `streamlink` 命令行工具可以处理的主播 URL（例如 YouTube 或 Kick）会归入 **Streamlink** 平台。其账号在每个主播上单独选择，而不是在平台或模板中选择；参见 [Streamlink 账号](../concepts/configuration.md#streamlink-accounts)。

### 直播信息查看

您可以使用内置播放器查看任何在线主播的可用直播流详情：
1. 前往 **侧边栏 (Sidebar)**。
2. 点击 **播放器 (Player)** 选项。
3. 在播放器视图中，您可以查看到所有可用的 **格式 (Formats)** (FLV, HLS)、**CDN** 以及 **画质 (Qualities)**。
4. 这可以帮助您验证当前配置（如所选账号）是否已生效，并成功解锁更高画质或不同格式。

### 多账号与平台能力限制

通过[账号配置](../concepts/configuration.md#账号配置与选择)选择固定账号或有序的优先/轮询账号池。平台拒绝某个账号的登录，或限流的账号因其[代理设置](../concepts/configuration.md#account-proxies)与下一个账号走不同连接时，才会自动故障切换。离线结果、内容限制和网络故障不能成为逐个尝试账号的理由，剩余账号共用的连接被限流时本次检查结束。无法区分这些失败的平台采用保守行为；不支持验证本身不代表账号不可用。

平台准入限制由所有凭据配置、匿名请求、显式 Cookie 解析、刷新和二维码登录共享。增加账号不会增加平台请求额度。这些请求（包括匿名请求）遇到限流时，该平台在同一连接上的所有请求都会暂停，时长为平台给出的重试间隔，未给出时为 60 秒。直连、系统代理以及每个已保存的代理各算一条[连接](../concepts/configuration.md#proxy-rate-limits)，无论由哪个设置或账号选择；每条连接开始暂停时都会发送注明该连接的“平台限流”通知。平台要求的间隔超过 15 分钟时会缩短为 15 分钟，因此单个响应不会让录制停止数小时。Bilibili 二维码/令牌请求、斗鱼二维码登录和会话续期，以及 SOOP 刷新给出的重试间隔同样受此上限约束。操作时限包含准入和刷新等待，修复与提取共同消耗有界尝试预算。

二维码生成时即绑定所选平台或凭据配置。改变目标需要重新生成二维码，并发轮询不会把一次登录写给两个账号。过期或重启可能需要新二维码；已在本地完成的回执可安全重复查询。每次生成或轮询请求最长 30 秒，放弃的二维码过期后一分钟内会清除其登录数据。状态界面区分管理性禁用、凭据无效及不支持验证。

托管播放将账号的 Cookie 和平台请求头保留在服务器。播放器获得流地址和经过授权的临时句柄，并始终通过服务器代理播放；代理只对该流自身的主机和来源页面的主机附加账号请求头，其他主机上的播放列表、分段、密钥和重定向不带这些请求头获取，代理也会拒绝将句柄用于其他主机。淘汰、重启、凭据/策略变更或过期均需要重新解析。刷新播放源时会先用同一账号续期；若句柄已过期或账号选择已更改，播放器会重新解析该来源，此时可能改用账号池中的其他账号。默认最多 1,024 个上下文，空闲 15 分钟、绝对存活 12 小时，因此持续播放最终也可能需要更新。显式原始 Cookie 是隔离的临时覆盖，不能与托管凭据请求混用。

托管 HLS 播放列表与其他经代理的播放列表采用相同方式重写。不支持 `EXT-X-DEFINE` 变量替换；接受低延迟查询参数 `_HLS_msn`、`_HLS_part` 和 `_HLS_skip`，但不向上游转发。

Twitch 凭据配置可以只填写访问令牌而不填写 Cookie。Twitch OAuth 令牌即凭据配置的访问令牌；抖音 `ttwid` 和斗鱼 `device_id`（即 `acf_did`）是凭据配置中的 Cookie，而不是平台选项。房间密码等非账号设置仍按正常规则继承。
