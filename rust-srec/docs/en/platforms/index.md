# Supported Platforms

rust-srec supports 12 streaming platforms with automatic stream detection and recording.

## Platform List

| Platform | URL Format | Protocol | Danmaku |
|----------|------------|----------|---------|
| [Bilibili](./bilibili.md) | `live.bilibili.com/{room_id}` | FLV/HLS | ✅ |
| [Douyin](./douyin.md) | `live.douyin.com/{room_id}` | FLV/HLS | ✅ |
| [Douyu](./douyu.md) | `douyu.com/{room_id}` | FLV | ✅ |
| [Huya](./huya.md) | `huya.com/{room_id}` | FLV/HLS | ✅ |
| [AcFun](./others.md#acfun) | `acfun.cn/live/{room_id}` | HLS | ❌ |
| [PandaTV](./others.md#pandatv) | `pandalive.co.kr/play/{id}` | HLS | ❌ |
| [Redbook](./others.md#redbook-小红书) | `xiaohongshu.com/user/profile/{id}` | HLS | ❌ |
| [Weibo](./others.md#weibo) | `weibo.com/u/{uid}` | HLS | ❌ |
| [Twitch](./twitch.md) | `twitch.tv/{channel}` | HLS | ✅ |
| [TikTok](./others.md#tiktok) | `tiktok.com/@{user}/live` | HLS | ❌ |
| [Twitcasting](./others.md#twitcasting) | `twitcasting.tv/{user}` | HLS | ✅ |
| [Picarto](./others.md#picarto) | `picarto.tv/{user}` | HLS/MP4 | ❌ |

## Common Configuration

Each platform can be configured at the platform level via **Settings** → **Platforms**.

### Authentication

Some platforms require cookies for:
- Higher quality streams
- Region-restricted content
- Subscriber-only streams

::: tip Stream Quality
If you are getting lower resolution than expected (e.g., 480p instead of 1080p), try adding cookies from a logged-in account. Many platforms restrict high-definition streams to authenticated users.
:::

See individual platform pages for authentication details.

### Stream Inspection

You can use the built-in player to inspect available stream details for any live streamer:
1. Go to the **Sidebar**.
2. Click on the **Player** option.
3. In the player view, you can see all available **Formats** (FLV, HLS), **CDNs**, and **Qualities**.
4. This helps you verify if your current configuration (like cookies) is correctly working to unlock higher qualities or different formats.
