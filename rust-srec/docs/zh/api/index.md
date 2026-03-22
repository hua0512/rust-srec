# API 概述

rust-srec 提供完整的 REST API 用于管理录制器的所有功能。

::: warning 正在建设中
详细的 API 指南目前正在重写中。请参考 Swagger UI 获取最新的接口信息。
:::

## 基础 URL

```
http://localhost:12555/api
```

## Swagger 文档

交互式 API 文档：

**[Swagger UI](/api/docs)** - `http://localhost:12555/api/docs`

::: tip API 测试建议
除了 Swagger UI，你也可以使用 [Postman](https://www.postman.com/) 或 [Insomnia](https://insomnia.rest/) 等工具测试 API。请记得在请求头中包含 `Authorization`。
:::

## 认证

除 `/auth/login` 和 `/health/*` 外，所有 API 端点都需要 JWT 认证。

```bash
# 登录获取令牌
curl -X POST http://localhost:12555/api/auth/login \
  -H "Content-Type: application/json" \
  -d '{"username": "admin", "password": "password"}'

# 在后续请求中使用令牌
curl http://localhost:12555/api/streamers \
  -H "Authorization: Bearer <access_token>"
```


## 通用响应格式

### 成功

```json
{
  "id": "uuid",
  "name": "...",
  ...
}
```

### 错误

```json
{
  "error": "错误信息",
  "code": "ERROR_CODE"
}
```

## HTTP 状态码

| 状态码 | 说明 |
|--------|------|
| `200` | 成功 |
| `201` | 已创建 |
| `400` | 请求错误 |
| `401` | 未授权 |
| `404` | 未找到 |
| `409` | 冲突（重复）|
| `500` | 服务器错误 |

## Session 分段时间戳

`GET /api/sessions/{id}/segments` 会为每个分段返回三个不同含义的时间戳：

- `created_at`：该分段开始录制的时间
- `completed_at`：该分段结束录制的时间
- `persisted_at`：该分段记录写入 SQLite 的时间

对于生命周期时间戳引入之前产生的历史数据，`created_at` 和 `completed_at` 可能为
`null`。这种情况下，`persisted_at` 仍然是可靠的数据库写入时间。
