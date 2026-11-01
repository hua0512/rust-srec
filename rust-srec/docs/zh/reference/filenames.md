# 文件名模板变量 {#文件名模板变量}

Rust-Srec 支持在 `output_folder` 和 `output_filename_template` 中使用两类占位符。

## 大括号变量 (Curly Brace Variables) {#大括号变量-curly-brace-variables}
这些变量将被替换为主播或会话相关的元数据。

| 变量 | 说明 |
|------|------|
| `{streamer}` | 主播显示名称，文件名中不允许的字符会替换为 `_` |
| `{title}` | 当前直播标题，同样经过字符替换 |
| `{platform}` | 平台名称 (如 bilibili) |
| `{session_id}` | 录制会话的唯一 ID (仅适用于 `output_folder`) |

## 百分号占位符 (Percent Placeholders, FFmpeg 风格) {#百分号占位符-percent-placeholders-ffmpeg-风格}
这些占位符将被替换为日期、时间或序列信息。

| 占位符 | 说明 |
|--------|------|
| `%Y` | 年份 (YYYY) |
| `%m` | 月份 (01-12) |
| `%d` | 日期 (01-31) |
| `%H` | 小时 (00-23) |
| `%M` | 分钟 (00-59) |
| `%S` | 秒数 (00-59) |
| `%i` | 分段序列号；仅适用于 `output_filename_template` |
| `%t` | Unix 时间戳 |
| `%%` | 字面量百分号 |

时间占位符按服务器本地时区渲染。无法识别的 `%` 序列保持原样。

`output_filename_template` 命名的是文件而不是路径，目录层级请放在 `output_folder` 中。`/`、`:` 等文件名中不允许的字符可能会被替换为 `_`。

示例：将 `output_folder` 设为 `/rec/{streamer}/%Y-%m-%d`，`output_filename_template` 设为 `%H-%M-%S_{title}`。

## 流水线目标路径占位符 {#流水线目标路径占位符}

流水线目标路径字段（例如 rclone 的 `destination_root` 和 copy/move 的
`destination`）支持 `{platform}`、`{streamer}`、`{title}`、`{streamer_id}`、
`{session_id}`，以及同样的 `%Y`、`%m`、`%d`、`%H`、`%M`、`%S`、`%t`、
`%%` 时间占位符。时间占位符会按服务器本地时区渲染。

rclone 默认使用任务创建时间展开时间占位符。将 `time_anchor` 设为
`session_start` 后，同一场直播的所有分段都会归入直播开始日期对应的文件夹，
即使直播跨过午夜也不会拆到次日目录。copy/move 在省略 `time_anchor` 时按执行时刻展开；需要确定性的锚点时可设为 `job_created` 或
`session_start`。

使用会话开始时间作为锚点时，请在文件名模板中保留 `%Y%m%d-%H%M%S` 或 `%t`。
如果多个会话把相同文件名写入同一个目标目录，rclone 以及本地 copy/move 操作
可能会根据具体操作和参数覆盖或跳过文件。
