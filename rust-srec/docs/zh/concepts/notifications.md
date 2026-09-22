# 通知系统

Rust-Srec 会记录通知事件，并可把选定事件发送到外部渠道。渠道送达语言、界面语言和仅浏览器通知彼此独立配置。

```mermaid
flowchart LR
  E[直播、下载、管道、系统与凭据事件] --> L[通知事件日志]
  E --> S[各渠道事件订阅]
  S --> P[优先级过滤]
  P --> R[重试与熔断]
  R --> C[外部渠道]
  E --> B[浏览器或桌面通知]
```

## 配置外部渠道

1. 打开**通知**，选择**添加渠道**。
2. 选择 Webhook、Telegram、Gotify 或 Email，并输入易识别的渠道名称。
3. 设置**最低优先级**、消息语言和**启用**状态。
4. 保存后执行测试操作；接收端收到测试消息才算验证成功。
5. 打开订阅管理器，选择应发送到该渠道的事件类型。

渠道设置与事件订阅相互独立。仅创建目标并不代表它会自动接收全部事件。

## 可用目标

| 目标 | v0.5 Web 界面 | 要求 |
|---|---|---|
| Webhook | 可创建、编辑、测试 | HTTPS 端点；可选请求头或认证 |
| Telegram | 可创建、编辑、测试 | 机器人令牌和 Chat ID |
| Gotify | 可创建、编辑、测试 | 服务器 URL 和应用令牌 |
| Email | 可创建、编辑、测试 | SMTP 主机、发件人与收件人；仅在中继允许时可不填凭据 |
| Discord | 可表示已有渠道；v0.5 表单禁用新建选择 | 后端/API 支持 Discord Webhook 设置 |
| Web Push | 按浏览器配置 | VAPID 密钥，以及 HTTPS 或 localhost |
| 实时轮询 | 按浏览器配置 | 应用标签页必须保持打开 |
| 桌面通知 | 仅桌面版 | 操作系统通知权限 |

因此 Discord 是受后端支持、但 v0.5 Web 界面新建受限的渠道，并非该版本界面中普遍可选的渠道。

## 优先级

API 与界面设置采用 0-10 数值优先级：

| 级别 | 值 | 示例 |
|---|---:|---|
| Low | 2 | 下播、分片进度、管道开始/完成 |
| Normal | 5 | 开播、下载完成、系统启动/关闭 |
| High | 8 | 下载错误/拒绝、管道失败、凭据刷新失败、百度网盘重新登录失败 |
| Critical | 10 | 致命错误、输出路径不可访问、空间不足、凭据无效 |

五种外部渠道共用启用状态和最低优先级规则。渠道测试通过同一路径发送 Normal
优先级的启动事件，因此禁用渠道或将最低优先级设为 High/Critical 时，测试也会被
过滤。使用 Rust 默认配置构造的邮件渠道最低优先级为 High，测试时请设置合适的阈值。

渠道会过滤低于其最低值的事件。在注明兼容的位置，API 也接受旧字符串 `low`、`normal`、`high`、`critical`；`info` 不是有效优先级。

## 语言

每个外部渠道可跟随服务器语言，也可覆盖为 `en` 或 `zh-CN`。`RUST_SREC_LOCALE` 设置后端生成消息的默认语言，与用户在 Web 界面选择的语言无关。

## Telegram 格式

Telegram 渠道支持 `HTML`、`Markdown`、`MarkdownV2`（不区分大小写），或使用空 `parse_mode` 发送纯文本。格式化消息使用 Telegram 显式实体：动态内容始终按原文显示，标题加粗，页脚使用斜体。长消息会在完整字符边界处截断，并采用保守的 4,096 UTF-16 单位上限。其他模式值会返回配置错误。参见 [Telegram Bot API](https://core.telegram.org/bots/api#sendmessage)。

## Web Push

生成 VAPID 密钥对，并在后端启动前设置三个变量：

```bash
docker run --rm ghcr.io/hua0512/rust-srec-vapid:v0.5.1
```

```dotenv
WEB_PUSH_VAPID_PUBLIC_KEY=...
WEB_PUSH_VAPID_PRIVATE_KEY=...
WEB_PUSH_VAPID_SUBJECT=mailto:operations@example.com
```

然后在**通知**页面为当前浏览器启用 Web Push、授予浏览器权限、选择优先级并发送测试。浏览器要求安全上下文；localhost 以外必须使用 HTTPS。

## 送达行为

外部渠道的临时失败会退避重试，反复失败的渠道会暂时停止发送，重试耗尽后保留失败记录。这些机制不保证送达。修改设置后应测试渠道，严重事件可配置第二个接收目标。

普通通知队列已满时，移除最早的待发送通知及其重试。队列上限为零会禁用普通投递，已开始的发送仍可能完成。渠道修改前已接收的通知继续使用原目标，重载失败时保留当前配置。

Web Push 使用独立的 2,048 事件队列。已满或不可用时，丢弃所有优先级的新事件，包括严重事件。`notification_stats.web_push_dropped` 记录丢弃数量，不自动重放；事件历史和外部渠道独立继续。超大载荷在发送前失败，关闭时会在期限内尝试发送已排队事件。

邮件立即投递，旧 `batch_window_secs` 设置被忽略。渠道实现和工作任务约定见[通知内部实现](../development/notifications.md)。

## 存储严重事件

- `out_of_space` 表示磁盘用量越过阈值。
- `output_path_inaccessible` 表示[输出根写入门](../development/architecture.md#输出根写入门)因跟踪目录不可写而阻止了新录制任务。

真实磁盘满在释放空间后可于后续探测自动恢复；失效 Docker 绑定挂载可能需要重启容器，参见[存储与容量](../operations/storage.md)。

<div id="队列与-web-push-投递" class="legacy-section">

此节内容已移至[通知系统](./notifications.md#送达行为).

</div>

<div id="后端通知接口" class="legacy-section">

此节内容已移至[通知内部实现](../development/notifications.md#后端通知接口).

</div>
