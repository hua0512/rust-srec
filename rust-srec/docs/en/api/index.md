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

`POST /api/auth/refresh` accepts `{"refresh_token":"..."}` and rotates the token
pair within the same login session. Store the replacement refresh token and
serialize refresh requests, including requests from other tabs. Replaying a
consumed token normally revokes all of the user's sessions; see [reuse policy](../operations/security.md#refresh-token-rotation).

`POST /api/auth/logout` accepts the same body and revokes only that session's
access and refresh tokens. You may also send the current session JWT in the
Authorization header: if the refresh row has already been cleaned up, this lets
the server revoke the JWT's own session. A supplied header must be valid; an API
key cannot act as this optional session credential. Authenticated
`POST /api/auth/logout-all` revokes every session for the user. Password changes
revoke existing sessions atomically and require a new login; successful
configuration imports also revoke login sessions and refresh tokens.

After upgrading to session-bound JWTs, old unbound access tokens are rejected.
Use an existing valid refresh token once to obtain a bound token pair, or sign in
again. `GET /api/auth/sessions` still lists active refresh-token records: IDs may
change on rotation, and sessions whose refresh token has expired or been cleaned
up may be absent even while their access token is valid.

Media, stream proxy, and download/log WebSocket routes use the Authorization
header first and accept `?token=` only when it is absent. A malformed or invalid
header never falls back to the query token. Read-only API keys may read health
details and recorded media; download/log WebSockets, stream proxy, and logging
configuration/archive routes require full keys. See [API Keys & MCP](./api-keys-mcp.md).

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

`details` is omitted when unavailable. Common statuses are `400` invalid input, `401` missing/expired credentials, `403` disabled account or required password change, `404` missing resource, `409` conflict, `422` validation failure, `429` too many failed login attempts, `500` internal failure, and `503` unavailable dependency.

`POST /api/auth/login` is rate limited per account and per source address. After five failed attempts for one account it answers `429` with code `TOO_MANY_REQUESTS` and a `Retry-After` header holding the number of seconds to wait; a successful login clears that account's count. A second, much looser budget (100 failures per window) caps password-hashing work per source address.

The source address is the peer of the TCP connection. `X-Forwarded-For` and `X-Real-IP` are not trusted, so **behind the bundled frontend container, nginx, or any other reverse proxy, every login is attributed to the proxy's address** — the per-address budget is a work cap, not a per-client lockout. Usernames longer than 128 characters — counted as characters, not bytes, so non-Latin names are not penalised — are rejected with `400` before either budget is consulted. A configuration import applies the same limit, so it cannot create an account that could never sign in. Both budgets are configurable; see [Configuration](../getting-started/configuration.md#login-throttling).

Clients should branch on HTTP status and `code`, not parse the human-readable `message`.

## Compatibility and Deployment

`POST /api/parse/batch` accepts at most 100 URLs; `DELETE /api/sessions/batch`
accepts at most 100 session IDs. Larger arrays receive `422` before any item is
processed. Split larger work into separate requests; empty arrays remain valid.
Login device descriptions are limited to 256 Unicode characters in logs and
newly stored tokens. Refresh also bounds descriptions inherited from older
tokens; existing rows are not bulk-rewritten.

Internal database, filesystem, stored-JSON and credential errors use generic
responses rather than embedding backend diagnostics. Safe request-validation
messages remain descriptive. Credential refresh retains its existing `400`
status and `requires_relogin` indication.

The v0.5 API paths are not prefixed with a version. Pin the backend image or binary version, keep the matching OpenAPI JSON with generated clients, and test before upgrading. Breaking behavior is described in the [Release Notes](../release-notes/).

For network deployments, terminate TLS at a reverse proxy, restrict the API to intended clients, and do not expose Swagger publicly unless it is needed. See [Security](../operations/security.md) and [Production Deployment](../operations/production.md).
