# API Keys & MCP

Rust-Srec supports long-lived **API keys** for programmatic access and ships a built-in **MCP (Model Context Protocol) server**, so AI assistants such as Claude and Cursor can query recordings, analyze danmu, and manage configuration with first-class tools.

## API Keys

API keys are an alternative credential to the short-lived JWT session tokens. They belong to the user who creates them and act with that user's permissions, limited by a per-key access level:

| Access level | REST API | MCP tools |
|---|---|---|
| `read_only` | `GET`/`HEAD`/`OPTIONS` requests only | Query tools only |
| `full` | All requests | All tools, including configuration changes |

Keys look like `srec_<64 hex characters>`. Only a SHA-256 hash is stored server-side; the raw key is shown exactly once at creation and cannot be recovered.

### Managing Keys

In the web UI, open **Settings → API Keys** to create, inspect, and revoke keys, and to copy a ready-made MCP client configuration.

Via the REST API (these three endpoints require a JWT session; an API key cannot manage API keys):

```bash
# Create (returns the raw key exactly once)
curl -X POST http://localhost:12555/api/auth/api-keys \
  -H "Authorization: Bearer <access-token>" \
  -H "Content-Type: application/json" \
  -d '{"name":"ai assistant","access_level":"read_only","expires_at":null}'

# List (metadata only, never the raw key)
curl http://localhost:12555/api/auth/api-keys \
  -H "Authorization: Bearer <access-token>"

# Revoke
curl -X DELETE http://localhost:12555/api/auth/api-keys/<key-id> \
  -H "Authorization: Bearer <access-token>"
```

`expires_at` is an optional Unix epoch timestamp in milliseconds; `null` means the key never expires. Revocation takes effect immediately.

### Using a Key

Send the key either as a Bearer token or in the `X-Api-Key` header:

```bash
curl http://localhost:12555/api/streamers \
  -H "Authorization: Bearer srec_..."

curl http://localhost:12555/api/streamers \
  -H "X-Api-Key: srec_..."
```

Restrictions that always apply, regardless of access level:

- API keys cannot call the key-management endpoints above, `POST /api/auth/change-password`, or `POST /api/auth/logout-all`.
- The WebSocket/media routes that authenticate via `?token=` query parameter (`/api/downloads`, `/api/logging`, `/api/media`, `/api/stream-proxy`) accept JWT access tokens only, so keys never appear in URLs or logs.
- Keys of a disabled user, or of a user in the forced-password-change state, are rejected.

## MCP Server

The backend serves the MCP **streamable HTTP** transport at:

```
http://<host>:<port>/api/mcp
```

Authentication uses the same header as the REST API (`Authorization: Bearer srec_...` or `X-Api-Key`). A `read_only` key can call query tools; tools that modify state respond with an error unless the key has `full` access.

### Client Configuration

Most MCP clients accept a JSON entry like this:

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

- **Claude Code**: `claude mcp add --transport http rust-srec http://localhost:12555/api/mcp --header "Authorization: Bearer srec_..."`
- **Cursor**: add the JSON above to `.cursor/mcp.json` (project) or `~/.cursor/mcp.json` (global).
- Any other client that speaks MCP streamable HTTP works the same way.

### Tool Groups

Tools mirror the REST API and run in-process against the same services, so validation and configuration hot-reload behave identically:

| Group | Examples | Purpose |
|---|---|---|
| `config_*`, `template_*`, `engine_*` | `config_get_global`, `config_update_global`, `template_create` | Read and change the global → platform → template → streamer configuration hierarchy |
| `streamer_*`, `filter_*` | `streamer_list`, `streamer_create`, `filter_create` | Manage monitored streamers and recording filters |
| `session_*` | `session_list`, `session_danmu_statistics`, `session_read_danmu` | Inspect recording sessions, segments, danmu statistics, and raw danmu XML (byte-paginated) |
| `pipeline_*`, `job_preset_*` | `pipeline_stats`, `pipeline_retry_job`, `pipeline_list_dags` | Observe and operate the post-processing pipeline |
| `notification_*` | `notification_list_channels`, `notification_test_channel` | Manage notification channels and subscriptions |
| `system_*`, `parse_url` | `system_health`, `parse_url` | Diagnostics and live-URL extraction |

For danmu analysis, prefer `session_danmu_statistics` (aggregated totals, rate time series, top talkers, word frequency) over reading raw XML; use `session_list_danmu_files` + `session_read_danmu` when the assistant needs actual chat text.

## Security Notes

- Treat API keys like passwords: store them in secret managers, never in shared configs or source control.
- Prefer `read_only` keys unless the assistant genuinely needs to change configuration, and set an expiry for keys used in experiments.
- Revoke keys immediately when a tool or machine is decommissioned; revocation invalidates in-flight caches within seconds.
- When `AUTH_DISABLED=true` (loopback-only development mode), `/api/mcp` is unauthenticated like the rest of the API.
