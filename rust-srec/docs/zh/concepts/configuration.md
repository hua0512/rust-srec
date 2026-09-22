# 配置层级

全局设置用于默认值，平台设置用于某个平台，模板用于一组主播，主播覆盖用于个别例外。

## 继承顺序

配置按以下顺序应用，优先级从低到高：

1. 全局
2. 平台
3. 模板（已分配时）
4. 主播

例如，全局输出格式为 `flv`，模板设为 `mp4`，使用该模板的主播就录制为 MP4。某个主播显式设为 `flv` 时，则使用 FLV。删除该覆盖后重新继承模板。

## 创建并使用模板

1. 创建包含一组主播共用设置的模板。
2. 创建或编辑主播时，为其分配模板。
3. 除非该主播需要不同的值，否则保留字段继承。
4. 修改模板即可更新这一组主播；正在录制的会话请参考下方生效规则。

模板也可提供平台专属覆盖。在对应平台上，这些覆盖优先于模板的通用设置。

## 需要注意的合并规则

- 已提供的标量值覆盖下层，省略的值继承下层。
- 管道覆盖替换整个管道，不逐步骤合并。
- 空的画质、格式和 CDN 偏好列表保留下层偏好；CDN 黑名单合并。
- 空 Cookie 字符串可能覆盖下层录制 Cookie。需要继承时应删除覆盖，不要输入空字符串。
- 平台选项逐键合并并忽略 `null` 覆盖；引擎覆盖使用 JSON Merge Patch，`null` 会删除相应键。

支持的 JSON 字段、凭据来源选择和 API 示例见[覆盖配置参考](../reference/configuration-overrides.md)。时间安排及夏令时规则见[录制时间安排](../guides/schedules.md)。

## 修改何时影响正在进行的录制 {#修改何时影响正在进行的录制}

通过界面或 API 修改设置不需要重启服务，但部分设置只在下一次下载时生效。

正在进行的下载会采纳：

- `min_segment_size_bytes`，以及分段管道和会话完成管道的定义——每个分段结束时都会重新从
  合并配置读取。在配置中清空某个管道并不会让已经在跑的会话也清空它：这次重读只会替换
  「有值」的管道。
- 保存全局设置后的 `max_concurrent_downloads`、排队刷新阈值、GPU 探测间隔和管道
  工作线程并发数。
- 新的监控间隔，应用于每个在线主播——但仅当 `streamer_check_delay_ms`、
  `offline_check_delay_ms` 或 `offline_check_count` 确实发生了变化。已经到达检查时间的主播
  会先完成那次检查，之后才应用新的间隔。

它不会采纳 `output_folder`、`output_filename_template`、`output_file_format`、
`max_download_duration_secs` 和 `max_part_size_bytes`。这些值在下载开始时解析一次，此后
在整个下载期间保持不变，分段轮换也不例外——轮换出的分段沿用开始时确定的基础文件名。修改
它们只对该主播的下一次下载生效，而不是当前下载的下一个分段。

已经排队的管道任务会继续沿用创建时携带的 DAG 定义。

<div id="_4-层配置层级" class="legacy-section">

此节内容已移至[配置层级](./configuration.md#继承顺序).

</div>

<div id="合并结果-mergedconfig" class="legacy-section">

此节内容已移至[配置解析](../development/configuration.md#合并结果-mergedconfig).

</div>

<div id="各项设置分别在哪一层配置" class="legacy-section">

此节内容已移至[覆盖配置参考](../reference/configuration-overrides.md#各项设置分别在哪一层配置).

</div>

<div id="合并规则-重要细节" class="legacy-section">

此节内容已移至[覆盖配置参考](../reference/configuration-overrides.md#合并规则-重要细节).

</div>

<div id="标量-高优先级层覆盖" class="legacy-section">

此节内容已移至[覆盖配置参考](../reference/configuration-overrides.md#标量-高优先级层覆盖).

</div>

<div id="离线判定和下载失败恢复共用同一个次数" class="legacy-section">

此节内容已移至[覆盖配置参考](../reference/configuration-overrides.md#离线判定和下载失败恢复共用同一个次数).

</div>

<div id="cookies-有值即覆盖-空字符串同样算有值" class="legacy-section">

此节内容已移至[覆盖配置参考](../reference/configuration-overrides.md#cookies-有值即覆盖-空字符串同样算有值).

</div>

<div id="流选择-由-streamselectionconfig-merge-合并" class="legacy-section">

此节内容已移至[覆盖配置参考](../reference/configuration-overrides.md#流选择-由-streamselectionconfig-merge-合并).

</div>

<div id="管道-高优先级层整体替换管道" class="legacy-section">

此节内容已移至[覆盖配置参考](../reference/configuration-overrides.md#管道-高优先级层整体替换管道).

</div>

<div id="平台附加项-json-浅合并-null-不覆盖" class="legacy-section">

此节内容已移至[覆盖配置参考](../reference/configuration-overrides.md#平台附加项-json-浅合并-null-不覆盖).

</div>

<div id="平台提取器选项-platform-extras" class="legacy-section">

此节内容已移至[覆盖配置参考](../reference/configuration-overrides.md#平台提取器选项-platform-extras).

</div>

<div id="凭据-cookies-refresh-token-单独解析" class="legacy-section">

此节内容已移至[覆盖配置参考](../reference/configuration-overrides.md#凭据-cookies-refresh-token-单独解析).

</div>

<div id="主播覆盖-streamer-specific-config" class="legacy-section">

此节内容已移至[覆盖配置参考](../reference/configuration-overrides.md#主播覆盖-streamer-specific-config).

</div>

<div id="引擎与提取器选择" class="legacy-section">

此节内容已移至[覆盖配置参考](../reference/configuration-overrides.md#引擎与提取器选择).

</div>

<div id="download-engine" class="legacy-section">

此节内容已移至[覆盖配置参考](../reference/configuration-overrides.md#download-engine).

</div>

<div id="extractor" class="legacy-section">

此节内容已移至[覆盖配置参考](../reference/configuration-overrides.md#extractor).

</div>

<div id="engines-override-仅模板层" class="legacy-section">

此节内容已移至[覆盖配置参考](../reference/configuration-overrides.md#engines-override-仅模板层).

</div>

<div id="热重载、缓存与更新事件" class="legacy-section">

此节内容已移至[配置层级](./configuration.md#修改何时影响正在进行的录制).

</div>

<div id="过滤器时区与边界" class="legacy-section">

此节内容已移至[录制时间安排](../guides/schedules.md#过滤器时区与边界).

</div>
