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

在[账号凭据配置](#凭据选择-json)和[代理连接](#proxy-route-json)出现之前，平台、模板和全局设置中的
`cookies` 与 `proxy_config` 字段同样已弃用。此版本首次启动时会转换数据库中这些字段的内容并将其清空，
导入旧备份时也会进行转换；这些字段将在未来版本中移除。
:::

### Cookies 不是配置字段 {#cookies-有值即覆盖-空字符串同样算有值}

Cookies 属于账号凭据配置，而不属于平台、模板或主播配置。某个作用域使用哪个账号，
由其[凭据选择](#凭据选择-json)决定。

平台和模板配置中旧的 `cookies` 字段已不再读取。仍然设置它的请求会以 HTTP 422
`COOKIES_REPLACED` 失败，避免客户端在不知情时丢失账号；值为 `null` 或空字符串时会被接受并忽略。
请改为通过 `/api/credentials` 把账号添加为凭据配置，再选中它。

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

::: tip 账号字段不属于平台附加项
账号材料保存在凭据配置中。任何一层的配置写入（包括 `platform_specific_config` 或
`platform_extras` 内部）只要包含 `cookies`、`refresh_token`、`access_token`、`oauth_token`、
`ttwid`、`device_id`、`session_cookies`、`reauth_config`、`last_cookie_check_date`、
`last_cookie_check_result`，或 SOOP 的 `username` 和 `password`，都会以校验错误被拒绝。
提取器只会从所选凭据配置获得这些值。房间密码（`stream_password`、Bigo 和 TwitCasting
的 `password`）属于内容设置，会保留。
:::

## 凭据来自账号凭据配置 {#凭据-cookies-refresh-token-单独解析}

Cookies、刷新令牌和访问令牌以及 SOOP 登录信息都保存在归属于平台的账号凭据配置中。
每个作用域的[凭据选择](#凭据选择-json)决定检查使用哪些凭据配置；所选配置的材料会交给
提取器、下载和弹幕采集使用，不会出现在 `MergedConfig` 或序列化的配置接口中。
参见[账号配置与选择](../concepts/configuration.md#账号配置与选择)。

## 播放器上游代理 {#player-upstream-proxy}

选择 **服务器代理** 后，网页和桌面播放与 URL 解析采用相同的连接方式，即该直播源生效的
[代理设置](../concepts/configuration.md#choosing-a-proxy)。已添加的主播使用其解析后的设置
（包含模板和主播的选择）；其他直播源 URL 在识别到平台时使用平台的设置，否则使用全局设置。
使用账号的播放会沿用其提取时的连接（包括账号自己的代理设置），因为平台可能按该地址签发流地址。
**直连** 由浏览器直接连接，不使用服务器的代理。

原始直播源 URL 会随 HLS 播放列表、分片和密钥请求保留，避免 CDN 地址选中不同配置。
配置更新作用于后续请求；连续的 FLV/MPEG-TS 连接需要重新加载播放器才能切换。
代理无法使用时播放会报错，不会回退到直连。网页播放需要同时升级前端和后端。

## 主播覆盖：`streamer_specific_config` {#主播覆盖-streamer-specific-config}

`streamer_specific_config` 是一个无类型 JSON 对象，未知的键会被忽略。

会影响 `MergedConfig` 的键：

- `output_folder`、`output_filename_template`、`output_file_format`
- `min_segment_size_bytes`、`max_download_duration_secs`、`max_part_size_bytes`
- `record_danmu`、`danmu_statistics`、`download_engine`、`extractor`、
  `offline_check_count`、`offline_check_delay_ms`
- `proxy_route`（连接方式对象，见[代理连接 JSON](#proxy-route-json)）
- `stream_selection_config`（JSON 对象）
- `download_retry_policy`（JSON 对象）
- `pipeline`、`session_complete_pipeline`、`paired_segment_pipeline`（JSON 对象）
- `platform_extras`（JSON 对象）

`credential_selection` 用于选择该主播的账号（见下文）。与其他各层一样，这里也会拒绝
`cookies`、`refresh_token` 等账号字段。

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

## 凭据选择 JSON

请求、响应和备份把选择作为 `credential_selection` 放在其所属的配置中：平台配置上（JSON 字符串）、
模板的 `platform_overrides[规范平台名]` 中，或主播的 `streamer_specific_config` 中。
这里的平台名区分大小写，须使用平台配置返回的精确名称。更新时省略策略字段会保留已存策略，
`{ "mode": "inherit" }` 则显式将该作用域重置为继承。平台、模板和主播的保存都遵循这一规则：
不含 `credential_selection` 的模板覆盖或主播配置会保留已存的选择。继承的作用域在返回时不含
`credential_selection`。

```json
{
  "credential_selection": {
    "mode": "pool",
    "credential_ids": ["profile-uuid-a", "profile-uuid-b"],
    "strategy": "priority",
    "failover": true,
    "max_attempts": 3
  }
}
```

其他策略为 `{ "mode": "none" }`、`{ "mode": "inherit" }` 及
`{ "mode": "fixed", "credential_id": "profile-uuid-a" }`。账号池支持 `priority`
或 `round_robin`，有序 ID 列表必须非空且不重复，总尝试次数为 1–10。
省略 `strategy`、`failover` 和 `max_attempts` 时，分别默认为 `priority`、`true` 和 `3`。
单成员池有效。
未知模式/字段会被拒绝。选择不存在的凭据或其他平台的凭据时，请求会以 HTTP 409
`CREDENTIAL_REFERENCE_INACCESSIBLE` 失败，并列出作出该选择的配置。
禁用的凭据允许保留引用，但执行时不可用。被选中的凭据不能删除：请求会以 HTTP 409
`CREDENTIAL_PROFILE_REFERENCED` 失败，并列出选择它的配置。主播的 URL 改为另一个平台时，
主播自己的选择会被移除，并在新平台上改为继承，即使请求重复提交原来的选择；主播被删除后
立即不再选择任何账号。
参见[选择和继承](../concepts/configuration.md#账号配置与选择)。

在 `streamlink` 平台上，只有主播可以保存选择，且只能是 `none` 或 `fixed`。在该平台上或模板的
`platform_overrides["streamlink"]` 中保存选择，或为 Streamlink 主播设置账号池，都会以 HTTP 422
`CREDENTIAL_SELECTION_PER_STREAMER` 失败。导入备份时，这类选择会改为设置到各个 Streamlink 主播上。
没有自有选择的 Streamlink 主播使用 `sites` 覆盖其 URL 的账号，没有则不使用账号。凭据配置在创建和更新时
接受 `sites`（更新时省略则保留原值）；只有 Streamlink 凭据配置接受它，无效的网站以 HTTP 422 失败，
已属于其他凭据配置的网站以 HTTP 409 `CREDENTIAL_SITE_TAKEN` 失败，并指明该凭据配置。对于 Streamlink 主播，
`GET /api/credentials/selection` 还会返回 `site`：主播的主机名、决定其继承账号的网站，以及网站覆盖该主机名的凭据配置。
参见 [Streamlink 账号](../concepts/configuration.md#streamlink-accounts)。

## 代理连接 JSON {#proxy-route-json}

每个作用域的代理设置是 `proxy_route`：它是全局、平台和模板配置中的字段，也是主播
`streamer_specific_config` 中的一个键。账号在创建、编辑以及通过扫码登录创建时也带有该字段。

```json
{ "proxy_route": { "kind": "proxy", "id": "proxy-uuid" } }
```

`kind` 取值为 `inherit`、`direct`、`system` 或 `proxy`；只有 `proxy` 需要 `id`，即某个
[已保存代理](../api/index.md#proxies)的 ID。未知的取值和多余字段会被拒绝。全局设置不能为
`inherit`（HTTP 422 `PROXY_ROUTE_INVALID`）。对账号而言，`inherit` 表示跟随使用它的录制的代理。
省略 `proxy_route` 或传 `null` 会保留已保存的设置，而 `{ "kind": "inherit" }` 会将其重置。
选择继承的主播在响应中不带 `proxy_route`。引用不存在的代理会以 HTTP 422 `PROXY_NOT_FOUND` 失败。
优先级见[选择连接方式](../concepts/configuration.md#choosing-a-proxy)。

旧的 `proxy_config` 对象已不再读取。仍然设置它的请求——无论在全局、平台或模板配置中、
`streamer_specific_config` 内，还是模板的 `platform_overrides` 内——都会以 HTTP 422
`PROXY_CONFIG_REPLACED` 失败，避免客户端在不知情时丢失代理；值为 `null` 或空字符串时会被接受并忽略。
保存代理功能推出之前写出的备份仍带有该字段，导入时会[自动转换](../operations/backup-restore.md#proxies-in-backups)。
