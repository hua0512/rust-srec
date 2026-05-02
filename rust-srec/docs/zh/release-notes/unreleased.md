# 更新日志

## `unreleased`

本次更新围绕两个独立主题：(1) **会话生命周期的 Hysteresis FSM 修复**——闭合"短暂断流后录制悄无声息地中止"的数据丢失漏洞，并对会话状态机的关键并发路径加上原子化保证；(2) **输出根写入门（output-root write gate）**——修复了一类"rust-srec 遇到文件系统问题（磁盘满、Docker 绑定挂载失效）必须重启容器才能恢复"的故障。同时引入了后端本地化的基础设施。

## 会话生命周期与 Hysteresis FSM 修复

> 背景：`refactor/session-hysteresis` 分支为录制会话引入了 `Recording → Hysteresis → (Recording | Ended)` 状态机，目标是吸收 FLV TCP 关闭、引擎瞬时错误等"歧义性"断流，让短暂的网络抖动不会立即结束录制。生产日志暴露出 FSM 在多个边界条件下被旁路或与下游产生竞态——本次更新逐层修复。

- **修复 FLV 干净断开后录制悄无声息地中止 1 小时以上**

  (kinetic（无畏契约）/ 2026-05-02：02:28 → 03:51 的 1.5 小时空白)

  当引擎报告 `Completed` + `CleanDisconnect` 时，`SessionLifecycle` 正确地将会话挂入 Hysteresis（80 秒静默期），但 `resume_from_hysteresis` 在窗口内检测到 LIVE 时短路，绕过了 `start_or_resume` 中负责派发 `MonitorEvent::StreamerLive` 出箱事件的原子事务。容器侧的 `handle_monitor_event::StreamerLive` 是 `download_manager.start_download(...)` 的**唯一**生产入口——因此恢复后会话在内存中显示 "Live"，但实际下载进程从未重启。

  通过把 `DownloadStartPayload`（streamer_url + streams + media_headers + media_extras）作为可选 sidecar 加在 `SessionTransition::Started` 上，并在容器中新增一个仅响应 `from_hysteresis: true` 的订阅器，从该 payload 合成一个 `MonitorEvent::StreamerLive` 然后走原有的 `handle_monitor_event` 路径——和全新会话启动复用同一段代码。`has_active_download` 幂等保护和新增的 `is_session_active` 防御让该订阅器对各种竞态都安全。

- **`Hysteresis → (Recording | Ended)` 退出路径加上原子 CAS**

  `enter_ended_state`（被 hysteresis 计时器触发、`on_offline_detected` 权威下线、`on_download_terminal` 权威结束三条路径调用）和 `resume_from_hysteresis`（被 `on_live_detected` 调用）此前可以并发地修改 `self.sessions` 与 `self.hysteresis`，引发：
  - 内存说 Recording 但 DB 说 Ended（end_time 已落库但内存被恢复路径覆盖）
  - 同一个 session_id 在毫秒之内连发 `Resumed + Started{from_hysteresis: true}` 与 `Ended` 三个 transition

  改用 `self.hysteresis.remove(session_id)` 作为 CAS 点（DashMap 的 per-key remove 是原子的）。胜出方继续走完整路径；失败方在快照对比中检测出 `was_in_hysteresis=true && claim=None`，立即返回——不写 DB、不更新内存、不广播。`resume_from_hysteresis` 失败时返回 `None`，`on_live_detected` 据此回退到 `start_or_resume`，自然产生一个新的 `Created` session_id（旧会话已 `Ended`）。

- **停止把瞬时 HTTP 404 误判为权威下线**

  (Minana呀 / 2026-04-29：3 个连续的 0 字节空会话)

  `OfflineClassifier` 此前把任意 mesio 404 都晋升为 `DefinitiveOffline { PlaylistGone(404) }`，绕过 hysteresis 直接结束会话。生产日志证明这在两类情况下严重过激：
  - **FLV 初始请求 404**：Douyu 等 CDN 在直播刚刚恢复时，分发的新令牌还没在边缘节点传播完，对该 URL 的首次 GET 返回 404——平台监控仍然报告 LIVE，状态机却把会话直接结束了。
  - **HLS 分片/播放列表中段 404**：滑动窗口剔除竞争、签名 URL 过期（部分平台返回 404 而非 403）、CDN 边缘失同步——任何一种都会导致一个仍然在直播的流被错误地判死。

  删除该分类规则。真正的下线信号现在通过两条更精确的渠道：
  - 连续 `Network` 失败（数量 = `offline_check_count`，窗口 = `count × interval_ms`，一切来自 scheduler 配置——与 `HysteresisConfig` 共用同一个真相源）
  - HLS 的 `#EXT-X-ENDLIST` 标签（mesio 已检测，本次 PR 把信号一直串到 `EngineEndSignal::HlsEndlist`，会话会立即权威结束，无需等 ~90 秒的 hysteresis 退场）

- **修复 Actor 层在干净引擎断开时虚发 `StreamerOffline`**

  (沈心 / 2026-05-01：单次直播中产生多张空会话卡片)

  `StreamerActor::handle_download_ended` 在所有 `DownloadEndPolicy::StreamerOffline | Stopped(_)` 路径上都会调用 `process_status(LiveStatus::Offline)`，监视器据此发出 `MonitorEvent::StreamerOffline` 权威事件——即使引擎只是干净 TCP 关闭、平台仍然在线。这条路径绕过了刚刚进入的 hysteresis 静默期。

  新增 `DownloadEndPolicy::Completed` 变体，专门承载"引擎干净断开但平台状态待定"的语义。`scheduler::service` 现在把 `DownloadTerminalEvent::Completed` 路由到这个新变体；Actor 在该分支只更新本地调度状态（短轮询恢复、`offline_observed` 计数+1），**不**主动推送下线给监视器。和 `DanmuStreamClosed` 变体一致的设计——FSM 自己负责判定权威。

- **三层防御消除 0 字节"幽灵会话"**

  即使经过上述修复，仍有少量边角场景可能产生空会话行（首次请求 404 之后没有重试、首字节前出错等）。我们补齐三层互补的防御：

  1. **API 过滤**（`SessionFilters::include_empty`，默认 `false`）——`GET /sessions` 默认排除 `total_size_bytes=0` 的已结束会话；活跃会话（`end_time IS NULL`）始终保留。诊断访问通过 `?include_empty=true` 与 `GET /sessions/:id` 仍然可用。
  2. **后台清理任务（`SessionJanitor`）**——周期性执行 `DELETE FROM live_sessions WHERE total_size_bytes = 0 AND end_time IS NOT NULL AND end_time < ?`。默认保留 5 分钟，扫描间隔 30 分钟。所有 4 个外键（`media_outputs` / `danmu_statistics` / `session_segments` / `session_events`）均已配置 `ON DELETE CASCADE`，子行随父行一起清理。任务幂等且崩溃恢复（SELECT 谓词是真相源；漏过的 tick 由下次 tick 接力）。
  3. **小段守护已存在**——`services::container` 的 `min_segment_size_bytes` 阈值删除了底下的文件，但此前并未清理对应的会话行。前两层弥补了这个缺口。

- **`OfflineClassifier` 的窗口与阈值改为从 scheduler 配置派生**

  此前是硬编码常量 `60 秒` 与 `阈值 2`。现在通过 `OfflineClassifier::from_scheduler(count, interval_ms)`：窗口 = `count × interval_ms`，阈值 = `max(count, 2)`（地板 2 仍然保护 Bilibili 风格的中段 RST 重连）。和 `HysteresisConfig::from_scheduler` 同源——运营人员只需要在一个地方调整"我对'下线'的判定窗口"。

- **HLS `#EXT-X-ENDLIST` 现在端到端**

  mesio 内部一直检测 ENDLIST，但信号在 `crates/mesio/src/hls/hls_downloader.rs` 的两处 `// TODO(hysteresis)` 注释处被丢弃。这次把 `HlsStreamEvent::EndlistEncountered` 通过新通道串到播放列表引擎、翻译为 `HlsData::EndMarker(Some(SplitReason::EndOfStream))`、由 rust-srec wrapper 通过 `consume_stream` 的新 `inspect` 闭包观察并最终升级为 `EngineEndSignal::HlsEndlist`。会话因此立即权威结束，无需等 hysteresis 退场。`hls-fix` 流水线的所有 4 个算子都已通过测试验证不会丢弃 EndMarker。

## 前端

- **会话详情页时间线 Tab 计数修正**——徽章此前只统计 `session.titles`，忽略了新增的 `session.events`。现在两者求和，正确反映 Tab 内实际渲染的条数。
- **`terminal-cause` 翻译消歧义**——会话时间线里的 `Completed/Failed/Cancelled/Rejected/...` 此前与流水线作业列表里的"已完成"等共享 lingui 翻译键，导致简体中文界面里 `原因：已完成` 出现在"待确认"卡片下方，语义错乱。改用 `<Trans context="terminal-cause">` 隔离翻译键，并提供更精确的中文：`Completed → 下载断开`、`Failed → 下载失败`、`Streamer Offline → 主播离线`、`Consecutive Failures → 连续失败` 等。
- **`Confirmed via backstop timer` 翻译修正**——简体中文从"通过备份计时器确认"（容易理解为"备份/冗余"）改为"等待恢复超时后确认"（更准确反映"hysteresis 静默期超时回退到 Ended"的语义）。

## 亮点

- 新增**输出根写入门**，提升录制文件系统故障的弹性 ([#508](https://github.com/hua0512/rust-srec/issues/508))

  当录制磁盘写满或目标挂载不可写时，rust-srec 现在会在文件系统边界上暂停录制，把状态通过 `/health` 暴露出来，发出一条包含可操作恢复说明的 critical 级通知；当文件系统恢复可写时会自动恢复——对于常见的磁盘满场景，**无需重启**。对于"通过宿主机清理破坏了 Docker 绑定挂载"的情况（例如通过宝塔面板的"移至回收站"操作挂载目录），写入门无法自动恢复（这是 Linux VFS 的限制，与 rust-srec 无关），但它现在会在一次监视周期内检测到问题、停止把日志淹没的级联重试风暴，并以明确的恢复说明提示用户重启容器。

  **替换了 #508 中可见的 40+ 次级联失败风暴**，只留下一个干净的 `Degraded` 状态和一条通知。新的[Docker 故障排查](../getting-started/docker.md#使用绑定挂载时如何释放磁盘空间)指南列出了如何避开挂载失效陷阱的安全清理方式。

- 在 ffmpeg 和 streamlink 引擎中新增**运行时 ENOSPC 检测**

  引擎的 stderr 读取任务现在会监控 `"No space left on device"` / errno `-28` / 退出码 228，并向下载管理器发出 `SegmentEvent::DiskFull` 事件，由管理器路由给写入门。这对"录制进行中磁盘才写满、今天的日期目录已存在"的常见场景至关重要——这种情况下启动前的 `ensure_output_dir` 钩子无法捕获故障。

- 启用 **`StreamerState::OutOfSpace` 的运行时写入**

  该状态此前存在于领域模型中，但从未在运行时被写入。现在当写入门阻塞某个主播时，状态会点亮为 `OutOfSpace`；写入门恢复时会自动清除。在主播列表中以停止状态徽章显示。

- 基于 `rust-i18n` 的**后端通知本地化**

  新增 `rust-srec/locales/{en,zh-CN}.yml` 文件，新增 `RUST_SREC_LOCALE` 环境变量。**所有通知事件**均已本地化（英文和简体中文）——包括直播上下线、录制生命周期、分段、流水线任务、系统告警和凭据事件。推送到外部接收端（Telegram、Gotify、Discord、Webhook、邮件、Web Push）的通知会自动遵循该语言设置。

- 新增 **`output_path_inaccessible` 通知事件**与前端订阅

  与已有的 `out_of_space` 磁盘预警不同：此事件仅在写入门**实际阻塞**录制时触发。优先级为 Critical。每次 `Healthy → Degraded` 切换**只发出一次**（而不是每次失败都发），通过每个启用的通知渠道推送一次。在订阅管理器中以独特的深红色显示。

- 新增**一次性启动探测**，针对已配置的输出根

  容器启动时，在完成主播加载之后、调度器启动之前，写入门会对每个已配置的根执行一次有界的 5 秒探测（通过 `spawn_blocking` + 超时），以便在第一秒就暴露已经坏掉的挂载点，而不是等到第一次监视周期触发下载尝试才发现。

## 新增环境变量

| 变量 | 用途 |
|---|---|
| `RUST_SREC_OUTPUT_ROOTS` | 以逗号分隔的绝对路径列表，作为写入门的输出根边界。未设置时，写入门会基于 `OUTPUT_DIR` 通过 2 段启发式推导一个根。 |
| `RUST_SREC_LOCALE` | 后端通知字符串的语言环境。影响所有通知事件（直播、录制、分段、流水线、系统、凭据）。支持：`en`、`zh-CN`，默认 `en`。 |

详见[配置说明](../getting-started/configuration.md#后端服务)。

## 重要重构

为了让写入门干净地落地，顺便完成了几项下载子系统的重构：

- **`ensure_output_dir` 从引擎中上移到管理器**。此前每个引擎（`ffmpeg`、`streamlink`）都在自己的 `start()` 中调用 `ensure_output_dir`，同时各自做错误包装。现在统一改为在 `DownloadManager::prepare_output_dir` 预启动钩子中调用一次，写入门也在同一位置介入。Mesio 和未来新增的引擎都能免费受益。

- **修复了遗留的 `EngineStartError::from(crate::Error)` bug**。旧实现把所有 I/O 故障都归类为 `DownloadFailureKind::Other`，丢失了 `std::io::ErrorKind` 信息。新实现会沿错误源链向下走，找到第一个 `std::io::Error` 并按其类型分类——重试决策和熔断器现在可以为所有 I/O 路径看到正确的失败类别。

- **`set_circuit_breaker_blocked` 重命名为 `set_infra_blocked(reason)`**（位于 `monitor/service.rs`）。新签名接受一个 `InfraBlockReason` 枚举，包含熔断器阻塞（原有行为）和输出根阻塞（新增）两个变体。两者走同一条持久化路径，审计记录集中在一处。这是一次**公开 API 重命名**，未保留废弃别名。

- **扩展 `reset_errors`**（仅文档修正——实际的重置路径通过 `StreamerManager::clear_error_state` 已经正确）。

- **`DownloadManager.output_root_gate` 字段改用 `OnceLock`**，在容器初始化时一次性晚绑定，之后读取无锁。这是必要的：服务容器的两个 builder 中有一个在 `DownloadManager` 之后才构造 `NotificationService`。

## 兼容性

- 无数据库迁移。
- 无前端破坏性 API 变更。`GET /sessions` 默认行为有所变化：`total_size_bytes=0` 且已结束的会话不再返回；通过 `?include_empty=true` 可以恢复以前的"全部返回"行为。`GET /sessions/:id` 不受影响。
- `set_circuit_breaker_blocked` 重命名为 `set_infra_blocked(reason)`——若有外部代码调用 monitor service（尚未发现），需要同步更新。
- `DownloadManagerEvent::DownloadRejected` 事件新增了 `kind: DownloadRejectedKind` 字段。通过 WebSocket 或广播 API 订阅事件流的外部程序会在 JSON 载荷中看到该字段；忽略它是安全的。
- `DownloadEndPolicy` 新增 `Completed` 变体（专门表达"引擎干净断开、平台状态待定"），原 `StreamerOffline | Stopped(_)` 分支保留并继续处理权威下线。所有调用 `handle_download_ended` 的非穷举匹配都安全；穷举匹配需要补一个 arm。
- `SessionTransition::Started` 新增 `download_start: Option<Box<DownloadStartPayload>>` 字段。已有的 `Started { .. }` 匹配（使用 rest pattern）无需调整；穷举字面量需要补 `download_start: None`。同时 `SessionTransition` 不再 `derive(PartialEq, Eq)`——`StreamInfo` 不实现 `Eq`，但代码库内既有用法都基于 `matches!`。
- `SessionFilters` 新增 `include_empty: Option<bool>` 字段，默认 `None`（即不返回空会话）。所有内部调用点都已更新。

## 重要重构（会话生命周期）

- `SessionLifecycle::on_live_detected` / `resume_from_hysteresis` / `enter_ended_state` 三条路径的并发协议改为以 `self.hysteresis.remove(session_id)` 作为单一原子 CAS 点。`resume_from_hysteresis` 现在返回 `Option<StartSessionOutcome>`，`None` 表示 CAS 失败，调用方回退到 `start_or_resume`。`enter_ended_state` 在 `was_in_hysteresis=true && claim=None` 的快照不一致时直接跳过——不写 DB、不更新内存、不广播。两条路径成对工作，保证一次 Hysteresis 退场只产生一次 `Ended` 或一次 `Resumed + Started{from_hysteresis: true}`，不会两者都广播。

- `OfflineClassifier` 的窗口与阈值从模块私有 `const` 改为通过 `from_scheduler(count, interval_ms)` 派生，与 `HysteresisConfig::from_scheduler` 同源。`OfflineClassifier::new()` 仍然存在（默认值 `60 秒 / 阈值 2`，匹配历史硬编码），只供测试夹具使用；生产构造点（`services::container` 中的两处）已迁移到 `from_scheduler`。

- `OfflineSignal::PlaylistGone(u16)` 变体被删除——`session_events.payload` 列里没有遗留生产数据使用该变体（在合入前确认过的 clean slate）。前端 `OfflineSignalSchema` 文档同步更新。

- `crates/pipeline-common::SplitReason` 新增 `EndOfStream` 变体，由 mesio 的 HLS 播放列表引擎在观察到 `#EXT-X-ENDLIST` 时发出，经 hls-fix 流水线全程透明传递（`segment_split` / `segment_limiter` / `defragment` / `analyzer` 已验证保留 reason），最终被 rust-srec 的 `consume_stream` 观察并提升为 `EngineEndSignal::HlsEndlist`。

- 新增 `services::session_janitor`——后台定期清理 `total_size_bytes=0 AND end_time<retention_cutoff` 的 `live_sessions` 行。Spawn 站点位于 `ServiceContainer::start()`，与会话生命周期订阅同一处。默认 `retention=5min`、`interval=30min`、`MIN_RETENTION=60s`（生产地板）。

## 备注

- **挂载失效场景无法在容器内部自动恢复**。重新绑定 Docker 挂载需要 `CAP_SYS_ADMIN` 以及对宿主机 mount namespace 的访问权限，非特权容器没有这些能力。写入门负责检测并提示用户重启；真正的自动恢复是部署侧的问题。[Docker 故障排查](../getting-started/docker.md#使用绑定挂载时如何释放磁盘空间)列出了从源头避免挂载失效的安全清理方式。
