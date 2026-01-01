# 支持的平台

rust-srec 支持 12 个直播平台，自动检测并录制直播流。

## 平台列表

| 平台 | URL 格式 | 协议 | 弹幕 |
|------|----------|------|------|
| [Bilibili](./bilibili.md) | `live.bilibili.com/{room_id}` | FLV/HLS | ✅ |
| [抖音](./douyin.md) | `live.douyin.com/{room_id}` | FLV/HLS | ✅ |
| [斗鱼](./douyu.md) | `douyu.com/{room_id}` | FLV | ✅ |
| [虎牙](./huya.md) | `huya.com/{room_id}` | FLV/HLS | ✅ |
| [AcFun](./others.md#acfun) | `acfun.cn/live/{room_id}` | HLS | ❌ |
| [PandaTV](./others.md#pandatv) | `pandalive.co.kr/play/{id}` | HLS | ❌ |
| [小红书](./others.md#redbook-小红书) | `xiaohongshu.com/user/profile/{id} or xhs.link/{id}` | HLS | ❌ |
| [微博](./others.md#weibo) | `weibo.com/u/{uid} or weibo.com/l/wblive/p/show/{id}` | HLS | ❌ |
| [Twitch](./twitch.md) | `twitch.tv/{channel}` | HLS | ✅ |
| [TikTok](./others.md#tiktok) | `tiktok.com/@{user}/live` | HLS | ❌ |
| [Twitcasting](./others.md#twitcasting) | `twitcasting.tv/{user}` | HLS | ✅ |
| [Picarto](./others.md#picarto) | `picarto.tv/{user}` | HLS/MP4 | ❌ |

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
