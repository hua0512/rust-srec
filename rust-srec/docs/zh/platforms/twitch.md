# Twitch

[Twitch](https://www.twitch.tv) 是提供游戏等内容的直播平台。

## URL 格式

```
https://www.twitch.tv/{频道名}
```

## 功能

- ✅ HLS 流
- ✅ 弹幕采集 (通过 IRC WebSocket)
- ✅ 多画质选项
- ✅ 支持订阅者专属直播 (需要 OAuth)

::: info
- **认证说明**：公开直播不需要认证。对于**订阅者专属**直播，请添加一个以 OAuth 令牌作为访问令牌的 Twitch 账号凭据配置（**设置** → **平台** → **Twitch** 的 **网络** 标签页）。
- **OAuth Token**：在 twitch.tv 登录后，复制浏览器 Cookie 中 `auth-token` 的值。可将其填为凭据配置的访问令牌，也可将完整的 `auth-token=…` Cookie 放入凭据配置的 Cookie 中。带 `oauth:` 前缀也可以，前缀会被忽略。该令牌不会自行过期；在该浏览器会话中退出登录或修改密码后才会失效。
- **账号检查**：Twitch 账号每天首次使用时以及在账号的 **…** 菜单中选择 **校验** 时会向 Twitch 检查一次。被 Twitch 拒绝的令牌会将账号标记为无效，账号池中的录制会切换到下一个账号。重新登录并替换令牌即可恢复；Twitch 令牌无法自动刷新。
- **Streamlink 提取器**：Twitch 主播的提取器设为 Streamlink 时，所选凭据配置的访问令牌会作为 API 授权请求头传给 Streamlink 的 Twitch 插件，凭据配置中的 Cookie 也会一并传入。
- **弹幕采集**：捕获聊天消息以及 "Bits" (打赏) 作为弹幕。
- **代理建议**：如果遇到卡顿或地区限制，建议使用代理（参考[代理](../concepts/configuration.md#proxies)）。
:::
