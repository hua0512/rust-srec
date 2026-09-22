# 覆盖配置参考

供 API 客户端和高级配置使用。继承示例见[配置层级](../concepts/configuration.md)。

## 各项设置分别在哪一层配置 {#各项设置分别在哪一层配置}

并非每个字段在每一层都可以设置。下面的列表对应解析器和构建器实际读取的内容。

- 仅全局层（基础默认值 + 运行时参数）：`auto_thumbnail`、并发/任务上限、调度延迟、日志过滤指令
- 仅平台层：`fetch_delay_ms`、`download_delay_ms`、`platform_specific_config`
- 仅模板层：`platform_overrides`、`engines_override`
- 仅主播层：`streamer_specific_config`（JSON 对象，见下文）

::: tip 各层的字段名并不相同
平台层和模板层存储的是 `stream_selection_config`（JSON），它会成为
`MergedConfig.stream_selection`；主播覆盖同样使用 `stream_selection_config` 这个键。
全局层的引擎和提取器默认值名为 `default_download_engine` 和 `default_extractor`，
而平台、模板和主播层使用 `download_engine` 和 `extractor`。
:::

## 合并规则（重要细节） {#合并规则-重要细节}

构建器的策略刻意保守：绝大多数字段都是“有值则覆盖”。

### 标量：高优先级层覆盖 {#标量-高优先级层覆盖}

对于大部分字符串/数字/布尔字段，只要高优先级层提供了值，就会替换低优先级层的值。

### 离线判定和下载失败恢复共用同一个次数 {#离线判定和下载失败恢复共用同一个次数}

`offline_check_count` 是连续离线信号的统一容忍次数。运行时会根据全局、平台、
模板和主播配置合并出每个主播的最终值，并将其同时用于：

- 确认主播离线前所需的连续离线状态检查次数
- 连续下载失败后让主播进入临时冷却的失败次数

默认值为 `3`。即使将 `offline_check_count` 设置为 `1`，下载失败阈值也最低为
`2`，以避免一次临时 CDN 或网络错误立即触发冷却。达到阈值后，冷却从 60 秒开始，
后续连续失败会让时间依次翻倍，最长为一小时。成功的状态检查或持续的下载进度会
清除累计失败状态。

`offline_check_delay_ms` 控制离线确认检查间隔及相关的会话迟滞窗口，不控制冷却时长。

这两个值最终都不会低于各自的下限：`offline_check_count` 至少为 `1`，
`offline_check_delay_ms` 至少为 `1000`。收敛发生在平台、模板和主播层，构建合并配置时还会
再做一次，因此即使全局层填了低于下限的值，也会在被读取之前得到修正。

::: warning 已弃用的兼容格式
序列化 `StreamerMetadata` 中的别名 `effective_offline_check_count` 和
`effective_offline_check_delay_ms` 已弃用。未包含 `backoff_threshold` 的持久化
`TransientError` 事件也已弃用。这些兼容格式将在未来版本中移除。新的集成必须使用
`offline_check_count` 和 `offline_check_delay_ms`，并在每个序列化的瞬时错误事件中包含
`backoff_threshold`。
:::

### Cookies：“有值即覆盖”（空字符串同样算有值） {#cookies-有值即覆盖-空字符串同样算有值}

Cookies 被当作单个可选字符串处理。只要高优先级层提供了 `cookies`，就会覆盖低优先级层。

::: tip Cookies 使用建议
不要把 cookies 设置成空字符串。空字符串同样算“有值”，会覆盖低优先级层，
实际效果是让兜底 cookies 失效。
:::

### 流选择：由 `StreamSelectionConfig::merge` 合并 {#流选择-由-streamselectionconfig-merge-合并}

流选择的合并有特殊语义：

- `preferred_formats`：仅当为 `Some(非空数组)` 时覆盖
- `preferred_media_formats`、`preferred_qualities`、`preferred_cdns`：仅当非空时覆盖
- `min_bitrate`、`max_bitrate`：仅当非零时覆盖
- `blacklisted_cdns`：取并集而不是替换，因此高优先级层只能新增排除项

这样模板只需声明自己关心的部分，不会丢掉平台层的默认值。

### 管道：高优先级层整体替换管道 {#管道-高优先级层整体替换管道}

管道由 JSON 解析为 `DagPipelineDefinition`。当某一层提供了管道时，它会整体替换
之前的管道定义（不存在逐步骤合并）。

模板还可以把管道写在 `platform_overrides[platform_name]` 里。它比模板自身顶层的
`pipeline`、`session_complete_pipeline` 和 `paired_segment_pipeline` 更具体，
因此解析器会在模板层之后再应用它们，结果是它们优先生效。

参见：

- [工作流指南](../concepts/pipeline.md)

### 平台附加项：JSON 浅合并，`null` 不覆盖 {#平台附加项-json-浅合并-null-不覆盖}

平台提取器选项通过 `platform_extras`（一个 JSON 数据块）传递，采用浅层对象合并：

- 如果两侧都是 JSON 对象，高优先级层的键会覆盖低优先级层的同名键。
- 高优先级层中值为 `null` 的键会被忽略（不产生覆盖）。
- 如果任意一侧不是对象，则高优先级层胜出。

具体实现位于 `platforms_parser::extractor::platform_configs::merge_platform_extras`。

::: tip 如何清除 platform_extras 中的键
`platform_extras` 使用浅合并，且忽略上层的 `null`。这意味着高优先级层无法通过 `null`
“取消”低优先级层的键，只能用一个非 null 的值覆盖它。
:::

## 平台提取器选项（`platform_extras`） {#平台提取器选项-platform-extras}

`platform_extras` 的来源和合并位置如下：

- 平台层：`platform_config.platform_specific_config`
- 模板层：`template_config.platform_overrides[platform_name]`
- 主播层：`streamers.streamer_specific_config.platform_extras`

每一层都会按层级顺序调用同一个合并函数。

::: tip 关于平台附加项中的凭据
平台、模板和主播记录都可能包含与凭据相关的键。每一层在并入 `platform_extras` 之前，
都会剥离 `refresh_token`、`access_token`、`session_cookies`、`last_cookie_check_date`
和 `last_cookie_check_result`，因此提取器配置绝不会携带凭据。
:::

## 凭据（`cookies` + `refresh_token`）单独解析 {#凭据-cookies-refresh-token-单独解析}

运行时会另外解析出 `credential_source`（挂在 `ResolvedStreamerContext` 上的附属数据），
用于认证和 refresh token 处理。它刻意不属于 `MergedConfig`，也不得通过序列化的配置
接口对外暴露。

优先级（由高到低）：

1. 主播覆盖：`streamer_specific_config.cookies`
   （可选附带 `streamer_specific_config.refresh_token` / `access_token`）
2. 模板：`template_config.cookies`
   （可选附带 `template_config.platform_overrides[platform].refresh_token` / `access_token`）
3. 平台：`platform_config.cookies`
   （可选附带 `platform_config.platform_specific_config.refresh_token` / `access_token`）

与 `MergedConfig.cookies` 不同，空字符串或只有空白字符的 `cookies` **不会**成为凭据来源：
该层会被跳过，继续考察下一层。`refresh_token` 和 `access_token` 都只会从 cookies 胜出的
那一层读取，因此主播没有配置自己的 cookies 时，主播层的 `refresh_token` 会被忽略。

平台层也可以在没有 cookies 的情况下产生凭据来源：对于 SOOP，
`platform_specific_config` 中配置了 `username` 和 `password` 时会得到一个凭据来源，
其 cookies 在首次使用时签发。

## 主播覆盖：`streamer_specific_config` {#主播覆盖-streamer-specific-config}

`streamer_specific_config` 是一个无类型 JSON 对象，未知的键会被忽略。

会影响 `MergedConfig` 的键：

- `output_folder`、`output_filename_template`、`output_file_format`
- `min_segment_size_bytes`、`max_download_duration_secs`、`max_part_size_bytes`
- `record_danmu`、`danmu_statistics`、`cookies`、`download_engine`、`extractor`、
  `offline_check_count`、`offline_check_delay_ms`
- `proxy_config`（JSON 对象）
- `stream_selection_config`（JSON 对象）
- `download_retry_policy`（JSON 对象）
- `pipeline`、`session_complete_pipeline`、`paired_segment_pipeline`（JSON 对象）
- `platform_extras`（JSON 对象）

由凭据子系统使用、不属于 `MergedConfig` 的键：

- `refresh_token`、`access_token`

两者都只从 cookies 胜出的那一层读取。它们属于在并入 `platform_extras` 之前会从每一层剥离的
五个键——`refresh_token`、`access_token`、`session_cookies`、`last_cookie_check_date` 和
`last_cookie_check_result`。

::: tip 无效 JSON 会被忽略
平台/模板/全局记录中的多数 JSON 字段都采用尽力而为的解析方式。解析失败时，解析器会
记录一条警告，并回退到默认值或上一层的值。`streamer_specific_config` 内部同理：
某个键的值结构不对时会被跳过并继承低优先级层，而不会让整次解析失败。

`platform_extras` 是例外。它的值会被原样取用，不做结构检查；而只要合并的任一侧不是对象，
合并就退化为“上层直接胜出”——因此在这里写标量或数组会替换掉从低优先级层累积下来的
extras，而不是被跳过。
:::

## 引擎与提取器选择 {#引擎与提取器选择}

### `download_engine` {#download-engine}

`download_engine` 是一个字符串，用于选择使用哪份下载引擎配置。它可以是：

- 内置引擎类型字符串（`ffmpeg`、`streamlink`、`mesio`）
- 存放在 `engine_configuration` 表中的自定义引擎配置 ID

两者都匹配不上时，会回退到下载管理器的默认引擎。

### `extractor` {#extractor}

`extractor` 用于选择由哪个提取器解析直播流地址。它独立于 `download_engine`，后者只决定
解析出的地址如何被拉取。可用取值：

- `auto`（默认）：按 URL 正则注册表分派
- `streamlink`：通过 Streamlink 解析

某一层存储 `NULL` 或空字符串表示不表达偏好，继承下一层的值。无法识别的名称同样只记录日志
并忽略，因此写错名字只会退化为继承，而不会导致解析失败。

### `engines_override`（仅模板层） {#engines-override-仅模板层}

模板可以提供 `engines_override`，它是一个 JSON 对象：

- `engine_id` -> `override_value`

下载开始时，下载管理器会检查所选引擎 ID 是否有对应的覆盖项。如果有，它会：

1. 载入基础引擎配置（内置类型用默认配置，自定义 ID 用数据库中的配置）
2. 以 JSON Merge Patch 语义应用覆盖：嵌套对象按键逐层合并，覆盖中值为 `null` 的键会被删除
3. 为该覆盖创建一个专用的引擎实例，其键包含覆盖内容的哈希，因此它的熔断器状态与未覆盖的
   引擎相互独立

::: tip 这里的 `null` 含义不同
`engines_override` 中把某个键设为 `null` 表示删除该键，而 `platform_extras` 会忽略上层的
`null`。
:::
