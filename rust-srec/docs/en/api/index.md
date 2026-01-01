# API Overview

rust-srec provides a comprehensive REST API for managing all aspects of the streaming recorder.

::: warning Under Construction
Detailed API guides are currently being rewritten. Please refer to the Swagger UI for the most up-to-date endpoint information.
:::

## Base URL

```
http://localhost:12555/api
```

## Swagger Documentation

Interactive API documentation is available at:

**[Swagger UI](/api/docs)** - `http://localhost:12555/api/docs`

::: tip API Testing
For exploring the API, you can also use tools like [Postman](https://www.postman.com/) or [Insomnia](https://insomnia.rest/). Just remember to include the `Authorization` header.
:::

## Authentication

All API endpoints (except `/auth/login` and `/health/*`) require JWT authentication.

```bash
# Login to get token
curl -X POST http://localhost:12555/api/auth/login \
  -H "Content-Type: application/json" \
  -d '{"username": "admin", "password": "password"}'

# Use token in subsequent requests
curl http://localhost:12555/api/streamers \
  -H "Authorization: Bearer <access_token>"
```


## Common Response Format

### Success

```json
{
  "id": "uuid",
  "name": "...",
  ...
}
```

### Error

```json
{
  "error": "Error message",
  "code": "ERROR_CODE"
}
```

## HTTP Status Codes

| Code | Description |
|------|-------------|
| `200` | Success |
| `201` | Created |
| `400` | Bad Request |
| `401` | Unauthorized |
| `404` | Not Found |
| `409` | Conflict (duplicate) |
| `500` | Internal Server Error |
