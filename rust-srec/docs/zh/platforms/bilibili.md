# Bilibili

[Bilibili](https://www.bilibili.com) 是中国领先的视频和直播平台。

## URL 格式

```
https://live.bilibili.com/{房间号}
```

## 功能

- ✅ FLV 和 HLS 流
- ✅ 弹幕采集
- ✅ 多画质选项
- ✅ 二维码登录支持

## 认证

### 二维码登录（推荐）

1. 进入 **设置 (Settings)** → **平台 (Platform)**
2. 选择 **Bilibili (Platform-bilibili)** → **网络 (Network)** 标签页
3. 点击 **扫码登录 (Qr login)**
4. 使用哔哩哔哩手机 App 扫码
5. 凭据自动保存

### 手动设置 Cookie

在 **平台配置** → **Bilibili** 中设置：

| Cookie | 必需 | 说明 |
|--------|------|------|
| `SESSDATA` | 是 | 会话令牌 |
| `refresh_token` | 是 | 用于自动刷新 Cookie 的令牌（可在浏览器 LocalStorage 的 `ac_time_value` 中找到） |
| `bili_jct` | 可选 | CSRF 令牌 |
| `DedeUserID` | 可选 | 用户 ID |

## 画质选项

| 画质 | 说明 |
|------|------|
| `10000` | 原画 |
| `400` | 蓝光 |
| `250` | 超清 |
| `150` | 高清 |
| `80` | 流畅 |

## 注意事项

::: warning
录制超清（1080P）及以上画质必须配置 Cookie
:::

::: info
- 部分直播需要登录才能获取高画质
- 大会员专属直播需要对应会员资格
- 当主播刚开播时，HLS 流可能会有延迟。
:::
