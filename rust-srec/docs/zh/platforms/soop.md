# SOOP

[SOOP](https://www.sooplive.co.kr) 是韩国直播平台，前身为 AfreecaTV。

## URL 格式

```
https://play.sooplive.co.kr/{channel_id}
https://play.sooplive.com/{channel_id}
https://play.afreecatv.com/{channel_id}
```

也支持包含直播编号的播放页 URL：

```
https://play.sooplive.com/{channel_id}/{broadcast_number}
```

## 功能

- ✅ HLS 流，多清晰度（`original`、`hd4000`、`hd`、`sd` 等）
- ✅ 支持需要登录的直播（账号或 Cookie）
- ✅ 支持密码房
- ✅ 弹幕采集（游客 WebSocket：公屏聊天与礼物）
- ❌ 发送弹幕（仅接收）

## 配置

平台选项在 **设置** → **平台** → **SOOP**。**公开房间无需任何配置**。

| 选项 | 默认 | 说明 |
|------|------|------|
| **直播间密码**（`stream_password`） | 空 | 密码锁房的默认密码。可在主播 URL 上用 `?pwd=...` 覆盖。 |

19+ 等需要登录的直播，需要一个填写了 SOOP 用户名和密码、或粘贴了 Cookie 的[账号凭据配置](../concepts/configuration.md#账号配置与选择)；长期受限频道建议优先使用 Cookie。详见下方的 **认证**。

### 密码锁房

1. 在平台（或主播）配置中填写 `stream_password`，**或**
2. 在主播 URL 后追加 `?pwd=密码`（URL 优先于平台默认）。

未提供密码时，锁房会以私密内容错误失败。

### 弹幕

为主播开启弹幕后：

- **会记录**：公开房间的公屏聊天与礼物（气球、巧克力、超级聊天、订阅等）；游客 WebSocket，无需账号。
- **不支持**：发送弹幕、完整进房提示体系。

登录门槛的成人房需要先有带 Cookie 的账号凭据配置（或填写用户名和密码以解锁视频）才能拿到聊天元数据。

::: info
- **清晰度**：各公开清晰度会在下载前用短时 AID 惰性解析。
- **认证**：为需要登录的直播添加一个填写了用户名和密码的账号凭据配置。会话 Cookie 会通过 private info 校验，失效时自动重新登录，并保存到该凭据配置供后续轮询复用；凭据配置中也可以手动粘贴 Cookie。新会话保存失败时检查仍会成功，并发送通知说明。

- **网络**：在不受支持的地区，SOOP 常返回 GDPR 地理限制占位响应（`RESULT=0` 且 `GDPR=true`）而不是真实直播信息。请使用韩国网络或[代理](../concepts/configuration.md#proxies)；rust-srec 会将其报告为地区错误，而不是“离线”。
- **录制**：播放列表中的 “preloading” 占位分段会被跳过，不会写入录制文件。
:::

## 独立账号凭据

每个 SOOP 凭据配置保存自己的登录信息；更新或重新登录一个账号不会修改其他账号的材料。
使用[固定选择或账号池](../concepts/configuration.md#账号配置与选择)决定录制使用哪个账号。
