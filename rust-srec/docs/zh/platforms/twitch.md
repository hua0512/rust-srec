# Twitch

[Twitch](https://www.twitch.tv) 是全球领先的游戏直播平台。

## URL 格式

```
https://www.twitch.tv/{频道名}
```

## 功能

- ✅ HLS 流
- ✅ 弹幕采集 (通过 IRC WebSocket)
- ✅ 多画质选项
- ✅ 支持订阅者专属直播 (需要 OAuth)

## 注意事项

- **认证说明**：公开直播不需要认证。对于**订阅者专属**直播，您必须在配置中提供 `oauth_token`。
- **OAuth Token**：您可以从浏览器的 Cookie 中获取或使用 Twitch Token 获取工具。格式通常为 `oauth:xxxxxxxxxxxxxx`。
- **弹幕采集**：捕获聊天消息以及 "Bits" (打赏) 作为弹幕。
- **代理建议**：如果遇到卡顿或地区限制，建议使用代理（参考 [Docker 配置](../getting-started/docker.md#代理配置)）。
