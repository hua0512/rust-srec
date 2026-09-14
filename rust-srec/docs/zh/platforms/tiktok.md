# TikTok

[TikTok LIVE](https://www.tiktok.com/live) 是 TikTok 的直播服务。

## URL 格式

```
https://www.tiktok.com/@{用户名}/live
```

`@用户名` 即频道 ID，主播下播后仍可继续监控。

## 功能

- ✅ FLV 与 HLS 流
- ✅ 多画质（原画、1080p60、720p、540p、360p、纯音频），含 AVC 与 HEVC 变体
- ✅ 弹幕采集（通过 webcast WebSocket 获取聊天、表情与礼物）
- ✅ 通过弹幕连接检测直播结束
- ❌ 需登录 / 仅订阅者可见的直播间

## 配置

平台选项位于 **设置** → **平台** → **TikTok**。公开直播间**无需任何配置**。

| 选项 | 默认值 | 说明 |
|------|--------|------|
| **API 模式** (`api_mode`) | `auto` | `auto` 优先使用无需签名的 `api-live/user/room` JSON 接口，失败时回退到解析直播页 HTML；`web` 与 `html` 强制使用对应方式。 |
| **强制原画质量** (`force_origin_quality`) | 关闭 | 当 TikTok 提供 `origin` 画质时只保留该画质。 |

::: info
- **认证**：无需 Cookie。弹幕采集会自动注册临时 `ttwid` 会话；浏览器 Cookie 为可选项。
- **地区限制**：部分直播间在某些地区不可用。若提示地区限制，请使用代理（见 [Docker 配置](../getting-started/docker.md#proxy)）。
- **弹幕**：采集聊天、纯表情消息和礼物。连击礼物只记录一次，取最终数量。
:::
