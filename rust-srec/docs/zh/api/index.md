# REST API

Rust-Srec 在 `/api` 下提供 JSON REST API。运行中的后端会为其准确构建版本生成权威 OpenAPI 文档。

- Docker 默认地址：[Swagger UI](http://localhost:12555/api/docs) 与 [OpenAPI JSON](http://localhost:12555/api/docs/openapi.json)
- 使用 `rust-srec/.env.example` 的源码环境：把以上链接中的 `12555` 改为 `8080`

Swagger 属于正在运行的 Rust-Srec 后端，并不位于 `docs.srec.rs` 文档域名下。

## 认证

大多数路由要求 `Authorization: Bearer <token>` 请求头。登录会返回短期访问令牌和有效期更长、会轮换的刷新令牌。

### 首次登录

初始账号为 `admin` / `admin123!`，在调用其他受保护接口前必须修改密码。

```bash
curl -X POST http://localhost:12555/api/auth/login \
  -H "Content-Type: application/json" \
  -d '{"username":"admin","password":"admin123!","device_info":"API quickstart"}'
```

响应包含以下字段：

```json
{
  "access_token": "...",
  "refresh_token": "...",
  "token_type": "Bearer",
  "expires_in": 3600,
  "refresh_expires_in": 604800,
  "roles": ["admin"],
  "must_change_password": true
}
```

使用返回的访问令牌替换默认密码：

```bash
curl -X POST http://localhost:12555/api/auth/change-password \
  -H "Authorization: Bearer <access-token>" \
  -H "Content-Type: application/json" \
  -d '{"current_password":"admin123!","new_password":"<unique-new-password>"}'
```

然后使用新密码重新登录，并使用新的访问令牌。`must_change_password` 为 `true` 时，其他受保护路由会返回 `403 PASSWORD_CHANGE_REQUIRED`。

### 调用受保护接口

```bash
curl http://localhost:12555/api/streamers \
  -H "Authorization: Bearer <access-token>"
```

### 刷新与撤销

`POST /api/auth/refresh` 接收 `{"refresh_token":"..."}` 并轮换令牌对。客户端应保存新刷新令牌并丢弃旧令牌。`POST /api/auth/logout` 使用同一请求体撤销当前会话；已认证的 `POST /api/auth/logout-all` 会撤销该用户的全部会话。

访问令牌、刷新令牌、Cookie 和平台凭据都应按敏感信息处理，不要写入日志或源代码仓库。

## 路由分组

| 前缀 | 用途 | 认证 |
|---|---|---|
| `/api/health` | 存活、就绪与依赖状态 | 混合；启用认证时仅 `/live` 公开 |
| `/api/auth` | 登录、刷新、退出、改密与会话 | 混合；以 Swagger 为准 |
| `/api/streamers` | 主播增删改查、检查、过滤器与批量操作 | Bearer 令牌 |
| `/api/config` | 全局/平台配置与备份导入导出 | Bearer 令牌 |
| `/api/templates` | 可复用配置模板 | Bearer 令牌 |
| `/api/engines` | 下载引擎实例 | Bearer 令牌 |
| `/api/sessions` | 录制会话 | Bearer 令牌 |
| `/api/pipeline` | 工作流、任务、预设、执行与产物 | Bearer 令牌 |
| `/api/notifications` | 通知渠道、订阅、偏好与事件 | Bearer 令牌 |
| `/api/credentials` | 平台凭据状态与刷新 | Bearer 令牌 |
| `/api/parse` | URL 与元数据解析 | Bearer 令牌 |
| `/api/downloads`、`/api/logging`、`/api/media`、`/api/stream-proxy` | 实时或媒体访问 | 因路由而异；查看 Swagger |

请求和响应字段应以生成的 OpenAPI 文档为准，不要仅根据本页摘要猜测。

## 错误

API 错误使用统一结构：

```json
{
  "code": "VALIDATION_ERROR",
  "message": "便于阅读的错误说明",
  "details": {}
}
```

没有额外信息时会省略 `details`。常见状态为：`400` 输入无效、`401` 凭据缺失或过期、`403` 账号禁用或要求改密、`404` 资源不存在、`409` 冲突、`422` 校验失败、`429` 登录失败次数过多、`500` 内部错误、`503` 依赖不可用。

`POST /api/auth/login` 会按账号和来源地址分别限流。同一账号连续五次失败后返回 `429`，`code` 为 `TOO_MANY_REQUESTS`，并在 `Retry-After` 响应头中给出需要等待的秒数；登录成功会立即清零该账号的计数。另有一个宽松得多的按来源地址配额（每窗口 100 次），用于限制密码哈希开销。

来源地址取自 TCP 连接的对端，且不信任 `X-Forwarded-For` 与 `X-Real-IP`，因此**在本项目自带的前端容器、nginx 或任何反向代理之后，所有登录都会被归到代理的地址上**——按地址的配额是开销上限，而不是针对某个客户端的锁定。超过 128 个字符的用户名会在两个配额生效之前直接返回 `400`（按字符而非字节计算，非拉丁文名称不会因此吃亏）。导入配置时也会应用同一限制，避免导入出无法登录的账号。两个配额均可配置，详见[配置](../getting-started/configuration.md#登录限流)。

客户端应根据 HTTP 状态和 `code` 分支，不要解析面向用户的 `message` 文本。

## 兼容性与部署

v0.5 API 路径没有版本前缀。请固定后端镜像或二进制版本，把配套 OpenAPI JSON 与生成客户端一同保存，并在升级前测试。破坏性行为会记录在[发布说明](../release-notes/)中。

网络部署应在反向代理处终止 TLS、限制 API 访问来源，非必要时不要公开 Swagger。参见[安全](../operations/security.md)和[生产部署](../operations/production.md)。
