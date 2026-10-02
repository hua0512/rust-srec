# 配置解析

`MergedConfigBuilder` 逐层应用配置，`ConfigResolver` 读取数据库记录并构建有效配置，`ConfigService` 缓存结果并向运行时服务广播更新。

用户配置的继承规则见[配置层级](../concepts/configuration.md)，JSON 字段见[覆盖配置参考](../reference/configuration-overrides.md)。

## 合并结果：`MergedConfig` {#合并结果-mergedconfig}

`MergedConfig` 是运行时用于监控、下载、弹幕和管道的最终配置。

主要字段（按用途分组）：

- 输出：`output_folder`、`output_filename_template`、`output_file_format`
- 限制：`min_segment_size_bytes`、`max_download_duration_secs`、`max_part_size_bytes`
- 弹幕：`record_danmu`、`danmu_statistics`
- 网络：`proxy_config`、`cookies`
- 引擎：`download_engine`、`extractor`、`download_retry_policy`、`engines_override`
- 流选择：`stream_selection`
- 管道：`pipeline`、`session_complete_pipeline`、`paired_segment_pipeline`
- 平台提取器选项：`platform_extras`
- 时序：`fetch_delay_ms`、`download_delay_ms`、`offline_check_count`、
  `offline_check_delay_ms`
- 会话体验：`auto_thumbnail`

部分设置属于全局运行时参数，不在 `MergedConfig` 之内，例如并发上限和日志过滤指令。

## 热重载、缓存与更新事件 {#热重载、缓存与更新事件}

`ConfigService` 在内存中缓存解析出的主播配置：

- TTL：1 小时（默认）
- 并发请求去重：同一主播同时只有一次解析在进行
- 解析硬超时：30 秒（避免进行中的条目卡死）

通过 API/界面修改配置时，该服务会让相关缓存条目失效，并广播 `ConfigUpdateEvent`，
以便调度器和各管理器作出响应。

典型的失效范围：

- `GlobalUpdated`：所有主播失效
- `PlatformUpdated`：该平台下的主播失效
- `TemplateUpdated`：使用该模板的主播失效
- `StreamerMetadataUpdated`：该主播失效
- `EngineUpdated`：所有主播失效（引擎的使用情况未被追踪）

::: tip 优先使用模板
把共用设置放进模板，而不是在每个主播上重复配置。之后修改一次模板即可让所有关联主播
重新解析配置，无需逐个主播修改。
:::

启动时，以及全局、平台或模板配置修改后，已解析的离线检查设置会同时为最多
十六个独立主播刷新。运行时事件处理器仍会等待当前批次完成，再处理下一个事件。
单个查询失败会保留该主播原有的元数据设置，不会阻止其他主播刷新；已标记为待
删除的主播仍不进入批量快照。这只改变刷新调度，不改变配置优先级或持久化恢复
的确认条件。

## 过滤器快照

监控检查共享不可变的过滤器快照，包括空结果。最多缓存 1,024 个主播，同时执行的仓库
加载最多为 16 个；同一主播的并发检查合并为一次加载，单次加载在 10 秒后超时。保持
过滤器顺序和跳过无效记录的行为；取消或失败会释放等待中的检查。

成功修改过滤器后，先使共享快照失效，再安排重新检查。导入、删除主播以及全局或广播
丢失后的协调，也会使相关快照失效。已持有快照的检查使用该版本完成；失效的旧加载不能
重新发布缓存。直接 SQL 或共享运行时之外的写入，最多会在 30 秒缓存 TTL 内保持不可见；
过期后的下一次检查会重新加载。

## 托管凭据的持久化与执行

策略只包含凭据 ID，不复制秘密。平台策略保存在 `credential_selection`，模板策略位于 `platform_overrides` 的精确规范平台名下，主播策略位于 `streamer_specific_config`。SQL NULL/字段缺失表示旧版行为；显式 `inherit` 跳过本层旧版材料，`none` 终止认证继承。缓存解析后的策略，在执行时读取当前秘密材料。

材料修订号防止刷新、健康检查和登录的过时结果写回；标签编辑另有乐观锁版本。策略、凭据和导入操作通过拥有独立生命周期的提交后工作发布，HTTP 请求取消不能撤销已提交配置的发布。导入以可移植所有者键解析目标，保留空闲 UUID，拒绝其他所有者占用的 UUID，不按名称/秘密猜测。整个引用关系的验证与所有者映射和凭据写入位于同一 SQLite 事务。

录制绑定持久化身份、材料修订号、策略代次及 epoch，不保存秘密。账号或修订号变化时必须重新提取完整媒体包，不能把当前 Cookie 附加到旧 URL。活动会话引用阻止物理删除。替换导入持久化清理意图，在事务外结束相关工作，引用消失后才清理凭据。配置备份排除运行时绑定、托管凭据健康状态、轮询游标、二维码会话及播放上下文。

平台共享准入覆盖旧版、原始和托管提取、刷新及二维码操作。逻辑时限包括准入、锁和修复等待，嵌套调用不会重复收费。只有明确的账号级错误允许故障切换，范围不明的限流仍按平台处理。协调仅限进程内；多个后端共享 SQLite 不保证公平轮询或唯一平台刷新。SQLite 修订号校验防止过时写入，不提供分布式执行协调。
