# 更新日志

## `unreleased`

本次更新围绕一个核心功能——**输出根写入门（output-root write gate）**——修复了一类"rust-srec 遇到文件系统问题（磁盘满、Docker 绑定挂载失效）必须重启容器才能恢复"的故障。同时引入了后端本地化的基础设施。

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
- 无前端 API 变更。
- `set_circuit_breaker_blocked` 重命名为 `set_infra_blocked(reason)`——若有外部代码调用 monitor service（尚未发现），需要同步更新。
- `DownloadManagerEvent::DownloadRejected` 事件新增了 `kind: DownloadRejectedKind` 字段。通过 WebSocket 或广播 API 订阅事件流的外部程序会在 JSON 载荷中看到该字段；忽略它是安全的。

## 备注

- **挂载失效场景无法在容器内部自动恢复**。重新绑定 Docker 挂载需要 `CAP_SYS_ADMIN` 以及对宿主机 mount namespace 的访问权限，非特权容器没有这些能力。写入门负责检测并提示用户重启；真正的自动恢复是部署侧的问题。[Docker 故障排查](../getting-started/docker.md#使用绑定挂载时如何释放磁盘空间)列出了从源头避免挂载失效的安全清理方式。
