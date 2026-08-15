# API 密钥与 MCP

Rust-Srec 支持用于编程访问的长期 **API 密钥**，并内置 **MCP（Model Context Protocol）服务器**，让 Claude、Cursor 等 AI 助手通过一流的工具查询录制、分析弹幕并管理配置。

## API 密钥

API 密钥是短期 JWT 会话令牌之外的另一种凭据。密钥属于创建它的用户，以该用户的权限行事，并受每把密钥的访问级别限制：

| 访问级别 | REST API | MCP 工具 |
|---|---|---|
| `read_only` | 仅允许经过批准的非敏感查询 | 录制会话/弹幕、聚合统计、通知事件与健康诊断 |
| `full` | 所有请求 | 所有工具，包括修改配置 |

密钥形如 `srec_<64 位十六进制字符>`。服务端只存储 SHA-256 哈希；原始密钥仅在创建时显示一次，之后无法找回。

### 管理密钥

在 Web 界面中打开 **设置 → API 密钥**，即可创建、查看、吊销密钥，并复制现成的 MCP 客户端配置。

通过 REST API 管理（以下三个端点要求 JWT 会话；API 密钥不能管理 API 密钥）：

```bash
# 创建（原始密钥只返回这一次）
curl -X POST http://localhost:12555/api/auth/api-keys \
  -H "Authorization: Bearer <access-token>" \
  -H "Content-Type: application/json" \
  -d '{"name":"ai assistant","access_level":"read_only","expires_at":null}'

# 列表（仅元数据，绝不包含原始密钥）
curl http://localhost:12555/api/auth/api-keys \
  -H "Authorization: Bearer <access-token>"

# 吊销
curl -X DELETE http://localhost:12555/api/auth/api-keys/<key-id> \
  -H "Authorization: Bearer <access-token>"
```

`expires_at` 为可选的 Unix 毫秒时间戳；`null` 表示永不过期。吊销立即生效。

### 使用密钥

密钥必须作为 Bearer 令牌发送：

```bash
curl http://localhost:12555/api/sessions \
  -H "Authorization: Bearer srec_..."
```

无论访问级别如何，以下限制始终生效：

- API 密钥不能调用上述密钥管理端点、`POST /api/auth/change-password` 或 `POST /api/auth/logout-all`。
- 只读密钥不能获取配置、主播覆盖配置、任务记录、预设、引擎、通知渠道、备份，以及其他可能包含已存凭据或运维机密的响应。
- 通过 `?token=` 查询参数认证的 WebSocket / 媒体路由（`/api/downloads`、`/api/logging`、`/api/media`、`/api/stream-proxy`）只接受 JWT 访问令牌，因此密钥不会出现在 URL 或日志中。
- 被禁用的用户、或处于强制改密状态的用户，其密钥会被拒绝。

## MCP 服务器

后端在以下地址提供 MCP **streamable HTTP** 传输：

```
http://<host>:<port>/api/mcp
```

认证方式与 REST API 相同（`Authorization: Bearer srec_...`）。`read_only` 密钥可以查看录制会话与弹幕、处理管道聚合统计、通知事件和系统健康状态。修改状态、向外部发起链接解析，以及暴露已存配置的工具需要 `full` 权限。

### 客户端配置

大多数 MCP 客户端接受如下 JSON 配置：

```json
{
  "mcpServers": {
    "rust-srec": {
      "url": "http://localhost:12555/api/mcp",
      "headers": {
        "Authorization": "Bearer srec_YOUR_API_KEY"
      }
    }
  }
}
```

- **Claude Code**：`claude mcp add --transport http rust-srec http://localhost:12555/api/mcp --header "Authorization: Bearer srec_..."`
- **Cursor**：将上述 JSON 加入 `.cursor/mcp.json`（项目级）或 `~/.cursor/mcp.json`（全局）。
- 其他支持 MCP streamable HTTP 的客户端配置方式相同。

### 工具分组

工具与 REST API 一一对应，并在进程内直接调用相同的服务，因此校验与配置热更新行为完全一致：

| 分组 | 示例 | 用途 |
|---|---|---|
| `config_*`、`template_*`、`engine_*` | `config_get_global`、`config_update_global`、`template_create` | 读取和修改「全局 → 平台 → 模板 → 主播」配置层级 |
| `streamer_*`、`filter_*` | `streamer_list`、`streamer_create`、`filter_create` | 管理监控的主播与录制过滤器 |
| `session_*` | `session_list`、`session_danmu_statistics`、`session_read_danmu` | 查看录制会话、分段、弹幕统计与原始弹幕 XML（按字节分页） |
| `pipeline_*`、`job_preset_*` | `pipeline_stats`、`pipeline_retry_job`、`pipeline_list_dags` | 观察与操作后处理管道 |
| `notification_*` | `notification_list_channels`、`notification_test_channel` | 管理通知渠道与订阅 |
| `system_*`、`parse_url` | `system_health`、`parse_url` | 诊断与直播链接解析 |

分析弹幕时优先使用 `session_danmu_statistics`（总数、速率时间序列、发言排行、词频统计）；只有当助手需要实际聊天文本时，才使用 `session_list_danmu_files` + `session_read_danmu`。

包含配置的工具组（`config_*`、`template_*`、`engine_*`、`streamer_*`、`filter_*`、处理管道/任务预设与执行详情工具、通知渠道/订阅工具，以及 `parse_url`）即使只读取也需要 `full` 密钥。这样可以防止只读助手获取平台 Cookie、处理器配置、通知凭据、直播访问数据或其他已存机密。

## 安全注意事项

- 像对待密码一样对待 API 密钥：存放在密钥管理器中，不要放进共享配置或源码仓库。
- 除非助手确实需要配置或运维详情，否则优先使用 `read_only` 密钥；实验用途的密钥建议设置过期时间。
- 工具或机器下线时立即吊销密钥；吊销完成后，过期的并发缓存填充无法授权后续请求。
- 当 `AUTH_DISABLED=true`（仅限回环地址的开发模式）时，`/api/mcp` 与其余 API 一样不做认证。
