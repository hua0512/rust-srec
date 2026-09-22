# 设置参考

在**设置 → 全局**中设置默认值。平台、模板和主播覆盖的关系见[配置层级](../concepts/configuration.md)。

## 全局设置 {#全局设置}

通过 **设置** → **全局** 访问。设置项分为以下几类：

### 文件配置 (File Configuration) {#文件配置-file-configuration}
| 设置 | 说明 | 默认值 |
|------|------|--------|
| `record_danmu` | 启用弹幕录制 | `false` |
| `danmu_statistics` | 每场直播的弹幕统计方式（见下文） | 默认值 |
| `auto_thumbnail` | 自动生成视频封面 | `true` |
| `output_folder` | 录制保存的基础目录（支持模板） | [按部署方式设置](../getting-started/configuration.md#选择录制目录)；已有数据库保留保存值 |
| `output_filename_template` | 录制文件的文件名模板 | 见[文件名模板](./filenames.md) |
| `output_file_format` | 默认输出格式 (mp4, flv 等) | `flv` |

### 弹幕统计 {#弹幕统计}

开启 `record_danmu` 的每场录制都会生成弹幕统计摘要：总数、活跃度时间线、最活跃的发言人、
高频词，以及平台上报礼物时的礼物排行。`danmu_statistics` 用于调整该摘要，可在全局设置，
也可按平台、模板和主播覆盖。未填写的字段保持默认值，因此 `{"top_talkers": 200}` 就是一份
完整的覆盖配置。

| 字段 | 说明 | 默认值 |
|------|------|--------|
| `enabled` | 是否计算摘要。关闭后仍会录制弹幕文件，只是不再计算和保存包含观众昵称的摘要。 | `true` |
| `top_talkers` | 每场列出的发言人和礼物赠送者数量（1–500） | `100` |
| `top_words` | 每场列出的高频词数量（1–500） | `50` |
| `top_gifts` | 每场列出的礼物名称数量（1–500） | `20` |
| `rate_bucket_secs` | 活跃度时间线的精度（秒）。超长直播会自动降低精度，因此场次页面会读取实际精度而不是假定。 | `10` |
| `talker_capacity` | 跟踪的不同发言人数（64–8192）。低于此值时计数精确；超过后为近似值，场次页面会用 `≈` 标注。 | `2048` |
| `word_capacity` | 跟踪的不同词数（64–8192），取舍相同 | `2048` |
| `gift_capacity` | 跟踪的不同礼物名称数量 | `256` |
| `extra_stop_words` | 在内置列表之外，额外从高频词图表中排除的词 | 无 |

超出范围的值会被收敛到最近的可用值而不是报错，且列出的数量不会超过跟踪的数量。

### 资源限制 (Resource Limits) {#资源限制-resource-limits}
| 设置 | 说明 | 默认值 |
|------|------|--------|
| `min_segment_size` | 保留分段的最小大小 | `1MB` |
| `max_download_duration_secs` | 分段的最大时长 | `0` (不限制) |
| `max_part_size` | 分段的最大大小 | `8GB` |

### 并发与性能 (Concurrency & Performance) {#并发与性能-concurrency-performance}
| 设置 | 说明 | 默认值 |
|------|------|--------|
| `max_concurrent_downloads` | 最大同时录制任务数 | `6` |
| `max_concurrent_uploads` | 最大同时上传任务数 | `3` |
| `max_cpu_jobs` | 最大并发 CPU 密集型任务数 | `0` (Auto / 自动) |
| `max_io_jobs` | 最大并发 I/O 密集型任务数 | `8` (0 = Auto / 自动) |
| `download_engine` | 录制引擎 (`ffmpeg`, `mesio` 等) | `mesio` |
| `queue_freshness_threshold` | 当某项录制在并发队列中等待时间超过该阈值时，rust-srec 会在启动前重新检查主播以刷新流地址和请求头。对签名 URL 会在几分钟内过期的平台尤其有用。设为 `0` 表示每次排队等待都刷新。 | `60 秒` |

由哪个提取器解析流地址是与 `download_engine` 相互独立的设置，此处不提供，需要在平台、模板或主播层级配置。参见[引擎与提取器选择](./configuration-overrides.md#引擎与提取器选择)。

### 网络与系统 (Network & System) {#网络与系统-network-system}
| 设置 | 说明 | 默认值 |
|------|------|--------|
| `streamer_check_interval` | 检查主播状态的间隔 | `60 Secs` |
| `offline_check_interval` | 检查离线状态的间隔 | `20 Secs` |
| `offline_detection_count` | 确认主播离线所需的连续检查次数。同一个最终配置值也决定连续下载失败多少次后进入临时冷却；下载失败阈值最低为 `2`。 | `3` |
| `enable_proxy` | 通过代理服务器路由流量 | `false` |

### 保留策略 (Retention) {#保留策略-retention}

| 设置 | 说明 | 默认值 |
|------|------|--------|
| `job_history_retention_days` | 已结束流水线、任务及上传历史的保留天数；`0` 表示永久保留 | `30` |
| `notification_event_log_retention_days` | 通知事件的保留天数；`0` 表示永久保留 | `30` |
| `output_retention_days` | 已结束会话输出的保留天数；`0` 表示禁用自动清理 | `0` |
| `output_retention_delete_files` | `false`：仅删除输出记录；`true`：同时删除有记录的本地文件 | `false` |

在 **全局设置 → 保留策略** 中配置输出保留。清理在启动时及每 30 分钟运行一次，跳过正在运行、最近更新或等待计划重试的处理任务；处理器占用文件时会推迟清理。文件删除还会跳过正在录制的目录以及最近修改或共享的文件，删除失败时保留记录以便后续重试。

**仅删除记录** 会保留磁盘上的文件。记录移除后，即使改为 **删除记录和文件**，也无法再自动删除这些失去记录的文件。此策略仅覆盖已登记的媒体输出，不包括任意文件、所有流水线派生文件、远程上传副本或会话分段历史。

### 流水线配置 (Pipeline Configuration) {#流水线配置-pipeline-configuration}
Rust-Srec 支持在不同阶段添加自定义流水线步骤（如：转码、通知、自定义脚本）：
- **Per-segment (分段后)**: 在每个视频分段录制完成后立即运行。
- **Paired Segment (合并对)**: 在视频和弹幕配对后运行。
- **Session Complete (会话结束)**: 在整个录制会话结束后运行。

::: info 目录组织
将 `output_folder` 设置为 `{streamer}/%Y-%m-%d` 可按主播分类并按日期建立子文件夹。`output_filename_template` 则可使用 `%H-%M-%S_{title}` 作为文件名。
:::
