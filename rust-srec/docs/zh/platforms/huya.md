# 虎牙

[虎牙](https://www.huya.com) 是中国领先的游戏直播平台。

## URL 格式

```
https://www.huya.com/{房间号}
```

## 功能

- ✅ FLV 和 HLS 流
- ✅ 弹幕采集
- ✅ 多画质选项
- ✅ 自动 CDN 选择

## 注意事项

- **认证说明**：通常**不需要**配置 Cookie。如有特殊需要，请在 **设置 (Settings)** → **平台 (Platform)** → **虎牙 (Platform-huya)** 中设置。
- **推荐格式**：虎牙支持 **FLV** 和 **HLS** 格式。建议优先使用 FLV 进行录制。
- **画质选择**：默认开启 `force_origin_quality` 以尝试获取最高画质。
- **协议说明**：支持 `use_wup` 和 `use_wup_v2`（默认）解析协议。请注意 **WUP 协议仅支持纯数字房间号**。
- **CDN 选择**：程序会自动尝试为您的网络环境选择最佳 CDN。
