# REST API

Rust-Srec exposes a JSON REST API under `/api`. The running backend generates the authoritative OpenAPI document for its exact build.

- Docker default: [Swagger UI](http://localhost:12555/api/docs) and [OpenAPI JSON](http://localhost:12555/api/docs/openapi.json)
- Source checkout using `rust-srec/.env.example`: change port `12555` in those links to `8080`

The Swagger link belongs to the running Rust-Srec backend, not to `docs.srec.rs`.

## Authentication

Most routes require an access token in the `Authorization: Bearer <token>` header. Login returns a short-lived access token and a longer-lived, rotating refresh token.

### First Login

The initial account is `admin` / `admin123!` and must change its password before it can use other protected endpoints.

```bash
curl -X POST http://localhost:12555/api/auth/login \
  -H "Content-Type: application/json" \
  -d '{"username":"admin","password":"admin123!","device_info":"API quickstart"}'
```

The response contains these fields:

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

Use the returned access token to replace the default password:

```bash
curl -X POST http://localhost:12555/api/auth/change-password \
  -H "Authorization: Bearer <access-token>" \
  -H "Content-Type: application/json" \
  -d '{"current_password":"admin123!","new_password":"<unique-new-password>"}'
```

Sign in again with the new password and use the new access token. While `must_change_password` is true, other protected routes return `403 PASSWORD_CHANGE_REQUIRED`.

### Call a Protected Endpoint

```bash
curl http://localhost:12555/api/streamers \
  -H "Authorization: Bearer <access-token>"
```

### Refresh and Revoke

`POST /api/auth/refresh` accepts `{"refresh_token":"..."}` and rotates the token pair. Store the newly returned refresh token and discard the old one. `POST /api/auth/logout` accepts the same body and revokes that session; authenticated `POST /api/auth/logout-all` revokes all sessions for the user.

Treat access tokens, refresh tokens, cookies, and platform credentials as secrets. Do not place tokens in logs or source control.

## Route Groups

| Prefix | Purpose | Authentication |
|---|---|---|
| `/api/health` | Liveness, readiness, and dependency status | Mixed; only `/live` is public when auth is enabled |
| `/api/auth` | Login, refresh, logout, password change, sessions | Mixed; see Swagger |
| `/api/streamers` | Streamer CRUD, checks, filters, and batch actions | Bearer token |
| `/api/config` | Global/platform configuration and backup import/export | Bearer token |
| `/api/templates` | Reusable configuration templates | Bearer token |
| `/api/engines` | Download engine instances | Bearer token |
| `/api/sessions` | Recording sessions | Bearer token |
| `/api/pipeline` | Workflows, jobs, presets, executions, and outputs | Bearer token |
| `/api/notifications` | Channels, subscriptions, preferences, and events | Bearer token |
| `/api/credentials` | Platform credential state and refresh operations | Bearer token |
| `/api/parse` | URL and metadata parsing | Bearer token |
| `/api/downloads`, `/api/logging`, `/api/media`, `/api/stream-proxy` | Realtime or media access | Route-specific; inspect Swagger |

Use the generated OpenAPI document for request and response schemas instead of guessing fields from this summary.

## Errors

API errors use one stable envelope:

```json
{
  "code": "VALIDATION_ERROR",
  "message": "A human-readable explanation",
  "details": {}
}
```

`details` is omitted when unavailable. Common statuses are `400` invalid input, `401` missing/expired credentials, `403` disabled account or required password change, `404` missing resource, `409` conflict, `422` validation failure, `500` internal failure, and `503` unavailable dependency.

Clients should branch on HTTP status and `code`, not parse the human-readable `message`.

## Compatibility and Deployment

The v0.5 API paths are not prefixed with a version. Pin the backend image or binary version, keep the matching OpenAPI JSON with generated clients, and test before upgrading. Breaking behavior is described in the [Release Notes](../release-notes/).

For network deployments, terminate TLS at a reverse proxy, restrict the API to intended clients, and do not expose Swagger publicly unless it is needed. See [Security](../operations/security.md) and [Production Deployment](../operations/production.md).
