# Mesio 引擎

Mesio 是 rust-srec 的**进程内 Rust 下载引擎**。与 `FFMPEG` 和 `Streamlink` 不同，它并非外部程序，而是运行在录制器内部——这也是它在三种引擎中 CPU 与内存占用最低、并支持 HLS 多线程下载的原因。本页介绍 Mesio 的内部工作原理；若需三种引擎的功能横向对比，请参阅[引擎](./engines.md)。

## 架构

单一的 `MesioDownloader` 负责选择协议（HLS 或 FLV）与来源，随后无论何种协议都产出一个统一的**下载会话（download session）**。该会话暴露两条流——保证交付的 **items**（媒体数据与终止标记）流，以及尽力而为的 **events**（进度／遥测）流——外加一个用于取消和可选指标的控制句柄。

```mermaid
flowchart LR
  SRC[流来源<br/>播放列表 / FLV 链接] --> MD[MesioDownloader<br/>协议探测 + 来源选择]
  FLV[FLV 下载器<br/>流式下载]

  subgraph HLS["HLS 反应堆引擎"]
    direction TB
    PW[PlaylistWatcher<br/>轮询播放列表] -->|快照| RX[调度反应堆 Reactor<br/>掌管分段状态、去重、重试预算<br/>受内存预算约束]
    RX -->|派发有上限的任务| FT[抓取任务<br/>下载 + 解密分段]
    FT -. AES-128，线程外 .-> CR[加密处理池]
    FT -->|载荷| RX
    RX -->|有序输入| AS[SequenceAssembler<br/>按序号排序、初始化先于媒体、缺口策略]
  end

  subgraph SESS["下载会话（HLS 或 FLV）"]
    direction TB
    ITEMS["items — 保证交付的媒体 + 终止标记"]
    EVENTS["events — 尽力而为的进度 / 遥测"]
    HANDLE["handle — 取消 + 可选指标"]
  end

  MD -->|HLS| PW
  MD -->|FLV| FLV
  AS --> SESS
  FLV --> SESS
  ITEMS --> REC[录制器<br/>可选媒体修复 + 分段写入]
  EVENTS --> UI[界面中的进度与统计]
```

## HLS 反应堆引擎

对于 HLS，全部下载状态都集中在一处——**调度反应堆（Scheduler Reactor）**。它掌管：

- **分段身份与去重**：播放列表刷新轮换鉴权令牌时分段不会被重复抓取（例如 Twitch 签名链接，可剥离轮换的查询参数，使同一分段在多次刷新间被识别）。
- **重试预算**：包括将下载途中过期的签名链接透明切换到较新的链接重试。
- **有上限的并发抓取任务**：其进行中的下载、解密处理与输出缓冲各自受明确的内存预算约束——因此高速流或加密流不会再让内存无限增长。

解密在独立的**加密处理池**中运行，位于调度循环之外，因此即便加密分段突发涌入也能保持响应，而不会堆积。随后 **SequenceAssembler** 保证输出有序、fMP4 初始化分段先于依赖它的媒体写入（避免编解码不匹配导致的损坏），并在分段掉出直播窗口时以明确的缺口取代无声卡死。

## 下载会话

Mesio 的 HLS 与 FLV 下载器共用这同一套会话模型，因此进度上报、重试处理与取消行为在两种协议间表现一致。**items** 流是权威的——它承载录制器所依赖的媒体与终止标记——而 **events** 流则是尽力而为的遥测，用于在界面中呈现进度与统计；**handle** 则负责取消与可选的性能指标。

## 媒体修复流水线

下载会话与媒体修复是两个独立层次。启用流处理和对应的 Mesio 修复开关时，下载数据会先经过修复再交给写入器；否则会直接进入写入器。

- **FLV 修复**会补全缺失的容器结构、修正时间戳不连续、按配置过滤重复数据，并在流头不兼容时开始写入新文件，同时维护快速定位所需的元数据。处理缓冲区有明确上限，因此损坏或异常的流不会导致内存无限增长。
- **HLS 修复**会保证分片 MP4 初始化数据的写入顺序，检查已交付的 TS 与 fMP4 分段是否出现不兼容的格式变化，必要时开始写入新文件，并执行文件大小或时长限制。内存限制按数据量计算，而不只依赖排队分段的数量。

HLS 下载反应堆会按配置的缺口策略报告或跳过无法获取的媒体。修复流水线只会接收已交付的媒体以及明确的分割或终止标记；它无法重建源站从未交付的字节，也不会对媒体载荷进行转码。

## Mesio 独占功能

Mesio 专属的处理选项记录在[引擎](./engines.md)页面：

- [FLV 一致性修复](./engines.md#_2-flv-一致性修复) —— 修复 FLV 结构与时间戳，并在不移动文件后续内容的前提下完成 AMF 元数据。
- [原始数据模式](./engines.md#_3-原始数据模式-raw-data-mode) —— 不做包解析，直接把流字节写入磁盘，以获得绝对最低的 CPU／内存开销。
- [HLS 一致性修复](./engines.md#_4-hls-一致性修复-mesio-独占) —— 在写入前保护分段结构，并在不连续或有意义的流变化处轮换输出。

> [!NOTE]
> 完整的引擎设计与源码放在一起，见 [`crates/mesio/docs/HLS_ENGINE_ARCHITECTURE.md`](https://github.com/hua0512/rust-srec/blob/main/crates/mesio/docs/HLS_ENGINE_ARCHITECTURE.md) 与 [`crates/mesio/docs/DOWNLOADER_ENGINE_ARCHITECTURE_PLAN.md`](https://github.com/hua0512/rust-srec/blob/main/crates/mesio/docs/DOWNLOADER_ENGINE_ARCHITECTURE_PLAN.md)。
