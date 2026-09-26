# 斗鱼

[斗鱼](https://www.douyu.com) 是中国主要的游戏直播平台。

## URL 格式

```
https://www.douyu.com/{房间号}
```

## 功能

- ✅ FLV 流
- ✅ 弹幕采集
- ✅ 多画质选项（通过 `rate` 进行选择）
- ✅ 支持 CDN 选择
- ✅ 互动玩法（互动直播）检测
- ✅ Android App 提取与 AVC / HEVC 编码偏好

::: info
- **认证说明**：App 提取使用匿名播放。浏览器登录 Cookie 不用于 App 播放认证；有效的 `acf_did` 可作为设备 ID，但 `device_id` 设置优先。需要浏览器认证的房间可选择已弃用的 Web 方式，并在 **设置** → **平台** → **斗鱼** 中配置 Cookie，但不保证能访问受限直播。
- **推荐格式**：斗鱼主要使用 **FLV** 格式进行直播录制。
- **画质控制**：可以通过 `rate` 设置来选择画质（0 为原画）。
- **CDN 切换**：如果录制出现卡顿，可以在配置中指定首选 CDN。
- **互动玩法**：可以通过 `disable_interactive_game` 配置自动跳过“互动玩法”的录制。
:::

## 提取设置

可以在斗鱼平台设置中配置以下选项，也可以在模板或主播配置中覆盖。未设置的选项继承上一配置层。

| 选项 | 可选值 / 默认值 | 行为 |
| --- | --- | --- |
| `api_mode` | `app`（默认）、`web` | App 使用 Android 播放接口；Web 保留旧提取实现，已弃用。 |
| `rate` | 非负整数，默认 `0` | 请求画质：`0` 原画、`3` 超清、`2` 高清、`1` 标清。可用档位取决于房间。 |
| `cdn` | App 默认 `hw`；Web 默认 `ws-h5` | App 示例：`hw`、`tct`、`hs`、`ws`。App 请求会自动移除已有的 `-h5` 后缀。 |
| `codec` | `avc`（默认）、`hevc` | 优先选择 H.264/AVC 或 H.265/HEVC。HEVC 不可用时回退到 AVC。 |
| `device_name` | 默认随机机型 | Android 机型，如 `OnePlus 12`。未设置时生成 `ABC-DE12` 格式的机型。 |
| `os_version` | 默认 `"14"` | 与机型一起用于生成 App User-Agent 的 Android 版本。 |
| `device_id` | 默认未设置 | 指定 32 位字母数字或 ASCII UUID 格式的 DID，优先于 `acf_did` 和来源设置。 |
| `device_id_mode` | `local`（默认）、`server`、`default` | 未指定 DID 且无有效 Cookie 时，分别选择本地生成、向斗鱼注册或使用固定兼容 DID。 |
| `only_audio` | `false`（默认） | 通过旧 Web 方式请求不含视频的 AAC 音频，不受 `api_mode` 影响，忽略编码偏好。 |
| `request_retries` | 默认 `3`，最小有效值 `1` | 房间元数据及 App 播放请求的最大尝试次数。 |
| `disable_interactive_game` | `false`（默认） | 将互动玩法房间视为未开播。 |

平台专属 JSON 示例：

```json
{
  "api_mode": "app",
  "rate": 0,
  "cdn": "hw",
  "codec": "hevc"
}
```

斗鱼可能调度到其他 CDN 或降低请求画质。解析后的直播流会报告实际返回的画质档位、CDN 和编码。App 播放失败时不会自动切换到 Web；如有需要，请明确选择 `web`。上述仅音频模式是兼容性例外。

同一提取器的 App 机型、生成的 User-Agent 和设备 ID 在重试及切换 CDN 时保持一致。设备设置不影响 Web 提取。服务端注册需要联网，超时为 5 秒，同一提取器会复用结果；注册失败会报错，不会静默切换身份。如需跨提取器实例或程序重启保持身份，请配置 `device_id` 或有效的 `acf_did` Cookie。

默认的本地 DID 使用 Android 客户端基于毫秒时间戳的 MD5 算法。`device_id_mode: "default"` 使用 `10000000000000000000000000001511`。
