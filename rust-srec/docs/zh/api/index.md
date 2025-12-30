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
