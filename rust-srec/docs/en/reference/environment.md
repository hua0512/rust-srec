<script setup>
import { withBase } from 'vitepress'
</script>

# Environment Variables {#environment-variables}

The following environment variables can be configured in your <a :href="withBase('/env.example')" download=".env.example">.env</a> file.

## General {#general}
| Variable | Description | Default |
|----------|-------------|---------|
| `TZ` | Container timezone | `UTC` |
| `VERSION` | Docker image version tag | `latest` |

## Paths {#paths}
| Variable | Description | Default |
|----------|-------------|---------|
| `DATA_DIR` | Directory for application data | `./data` |
| `CONFIG_DIR` | Directory for platform configuration files | `./config` |
| `OUTPUT_DIR` | Initial recording folder when the standalone backend creates a fresh database; also watched by startup and disk-space health probes. Under Docker Compose this is the host bind-mount directory, while the container receives `OUTPUT_DIR=/app/output`. Existing database settings are preserved. | `./output` |
| `LOG_DIR` | Directory for log files. A relative value resolves against the process working directory; the bundled system service sets it to `/var/log/rust-srec` instead, so log files are stored outside the state directory. See [Installation](../getting-started/installation.md). | `./logs` |
| `LOG_MAX_FILE_BYTES` | Maximum bytes per managed log segment, read at startup; integer from 1024 to 1073741824. Oversized records are truncated with a marker. | `16777216` (16 MiB) |
| `LOG_MAX_FILES` | Maximum managed log segments including the current file, read at startup; integer from 2 to 1024. Use consistent settings for shared `LOG_DIR`; see [retention limits](../operations/monitoring.md#logs). | `16` |

::: tip Initial and saved recording directories
The standalone backend initializes a fresh database's `output_folder` from `OUTPUT_DIR`, using `./output` when unset or blank. It resolves relative paths against the startup working directory and saves an absolute path. Docker Compose and the systemd unit provide `/app/output` and `/var/lib/rust-srec/output`, respectively.

If initial migrations or saving the output folder fail, the next start resumes initialization with the absolute path selected on the first attempt, even if `OUTPUT_DIR` or the working directory changes.

Later starts preserve the saved setting. Change it under **Settings** → **Global** → **Output Folder**, with optional overrides per platform, template, and streamer. The resolved path shown by the application is authoritative. An existing binary or system-service installation that still has `/app/output` needs this setting changed to a writable directory.

Keep `RUST_SREC_OUTPUT_ROOTS` aligned with the saved folder when explicit boundaries are configured. Discovery uses saved output settings and overrides; a stale `OUTPUT_DIR` value does not add a second probe location after initialization.
:::

## Shutdown {#shutdown}
| Variable | Description | Default |
|----------|-------------|---------|
| `RUST_SREC_SHUTDOWN_TIMEOUT_SECS` | Strict standalone-server shutdown deadline | `30` |
| `RUST_SREC_SHUTDOWN_FORCE_RESERVE_SECS` | Time reserved inside the deadline for forced process-tree containment; must be greater than zero and less than the total timeout | `2` |
| `RUST_SREC_CONTAINER_STOP_GRACE_PERIOD` | Docker Compose wait before external SIGKILL; keep longer than the backend deadline | `35s` |
| `RUST_SREC_RUNTIME_MARKER_PATH` | Dirty-generation marker retained after a forced or crashed runtime | Beside the SQLite database |

The standalone backend allows 30 seconds for shutdown by default and reserves the final two seconds for forced process cleanup. Set the total with `RUST_SREC_SHUTDOWN_TIMEOUT_SECS` and keep the force reserve positive and below that total. Docker's stop grace period must be longer than the backend deadline.

A forced or crashed run leaves a recovery marker beside the database. Later clean exits do not clear earlier unresolved recovery. Remove the marker only while the backend is stopped and after checking interrupted files. Exit status `124` means the hard deadline expired; `125` means process-tree termination could not be requested. See [runtime shutdown](../development/architecture.md#observability-health-and-shutdown) for signal and process-containment details.

## Network {#network}
| Variable | Description | Default |
|----------|-------------|---------|
| `API_BIND_ADDRESS` | IP address the backend API binds to | `0.0.0.0` |
| `API_PORT` | External port for the backend API | `12555` |
| `FRONTEND_PORT` | External port for the web interface | `15275` |
| `BACKEND_URL` | Internal URL for the frontend to reach the backend | `http://rust-srec:8080` |
| `HTTP_PROXY` | HTTP proxy server URL | - |
| `HTTPS_PROXY` | HTTPS proxy server URL | - |
| `NO_PROXY` | Comma-separated list of hosts to bypass proxy | - |

## Security & Auth {#security-auth}
| Variable | Description | Default |
|----------|-------------|---------|
| `JWT_SECRET` | Secret key for JWT signing (**Required** unless using the local-only opt-out below) | - |
| `AUTH_DISABLED` | Disable backend authentication for loopback-only local development | `false` |
| `API_CORS_ORIGINS` | Comma-separated exact browser origins (`scheme://host[:port]`) allowed to call the API cross-origin while authentication is disabled | Local dev server and desktop webview origins |
| `API_LOGIN_MAX_FAILURES` | Failed logins tolerated per account inside the window | `5` |
| `API_LOGIN_IP_MAX_FAILURES` | Failed logins tolerated per source address inside the window | `100` |
| `API_LOGIN_WINDOW_SECS` | Length of the failed-login window, in seconds | `900` (15m) |
| `JWT_ISSUER` | JWT issuer identifier | `rust-srec` |
| `JWT_AUDIENCE` | JWT audience identifier | `rust-srec-api` |
| `SESSION_SECRET` | Frontend session encryption secret (**Required**, min 32 chars) | - |
| `COOKIE_SECURE` | Set to `true` to force HTTPS-only cookies | (auto) |
| `MIN_PASSWORD_LENGTH` | Minimum length for user passwords | `8` |

The backend refuses to start without a non-empty `JWT_SECRET`. For local development only, authentication can be disabled by setting both `AUTH_DISABLED=true` and `API_BIND_ADDRESS=127.0.0.1` (or `::1`). The backend rejects this opt-out for wildcard, hostname, and non-loopback bind addresses.

While authentication is disabled, only the origins in `API_CORS_ORIGINS` may call the API from a browser; the default list covers `http://localhost:15275`, `http://127.0.0.1:15275`, `http://[::1]:15275`, `tauri://localhost`, and `http://tauri.localhost`. Set the variable to override it — entries must be exact origins with no trailing path, and malformed entries are skipped with a warning at startup. Requests from any other origin are refused with `403`, as are requests whose `Host` header is neither a loopback name nor the configured bind address. With authentication enabled the variable is ignored and any origin may send requests, because every protected route still requires a bearer token.

## Login throttling {#login-throttling}

`POST /api/auth/login` counts failed attempts in a sliding window and answers `429` with a `Retry-After` delay once a budget is spent. Two budgets apply to every attempt:

- **Per account** (`API_LOGIN_MAX_FAILURES`, default 5). A successful login clears it immediately.
- **Per source address** (`API_LOGIN_IP_MAX_FAILURES`, default 100). This limit is higher to allow for shared proxies. The source address is the peer of the TCP connection, and `X-Forwarded-For` is not trusted, so behind the bundled frontend container, nginx, or any other reverse proxy **every login arrives from the proxy's address**. Treat this budget as a cap on password-hashing work, not as a per-user lockout — while it is exhausted, everyone behind that proxy is throttled. Raise it if that matters more to you than the hashing cap; lower it only if browsers reach the backend directly.

Both share the window length set by `API_LOGIN_WINDOW_SECS`.

## Token Expiration {#token-expiration}
| Variable | Description | Default |
|----------|-------------|---------|
| `ACCESS_TOKEN_EXPIRATION_SECS` | JWT access token lifetime | `3600` (1h) |
| `REFRESH_TOKEN_EXPIRATION_SECS` | JWT refresh token lifetime | `604800` (7d) |

## Browser Notifications (Web Push / VAPID) {#browser-notifications-web-push-vapid}
| Variable | Description | Default |
|----------|-------------|---------|
| `WEB_PUSH_VAPID_PUBLIC_KEY` | VAPID public key (base64url, unpadded). Leave empty/unset to disable. | - |
| `WEB_PUSH_VAPID_PRIVATE_KEY` | VAPID private key (base64url, unpadded). Leave empty/unset to disable. | - |
| `WEB_PUSH_VAPID_SUBJECT` | VAPID subject (e.g. `mailto:admin@localhost`) | `mailto:admin@localhost` |

## Backend Service {#backend-service}
| Variable | Description | Default |
|----------|-------------|---------|
| `RUST_LOG` | Logging level (`trace`, `debug`, `info`, `warn`, `error`) | `info` |
| `DATABASE_URL` | SQL database connection string. The value shown is the one the Docker `.env` sets. Left unset the backend falls back to `sqlite:srec.db?mode=rwc`, relative to the working directory; the bundled system service sets `sqlite:///var/lib/rust-srec/rust-srec.db`. The runtime generation marker is derived from this URL and is stored beside the database file. | `sqlite:///app/data/rust-srec.db` (Docker) |
| `RUST_SREC_LOCALE` | Locale for backend-emitted notification strings. Affects every notification event — stream online/offline, download lifecycle, segments, pipeline jobs, system alerts, credential events. Supported: `en`, `zh-CN`. | `en` |
| `RUST_SREC_OUTPUT_ROOTS` | Comma-separated list of **absolute** paths to treat as output-root boundaries for the write gate. If unset, the gate uses a heuristic that takes the first **two named components** of each resolved output path (e.g. `/rec/huya` for `/rec/huya/X/20260415`, `/home/user` for `/home/user/recordings/X/20260415`). Two named components is the smallest safe default — it avoids accidentally sharing a gate key across unrelated users in `/home/...` layouts. For a single-mount `/rec`-style layout where you want one gate key per mount (and therefore one aggregated notification on failure instead of one per platform), set this explicitly: `RUST_SREC_OUTPUT_ROOTS=/rec`. | - |

The heuristic groups deep paths under broader keys: `/var/lib/rust-srec/output` uses `/var/lib`. Startup discovery tests a concrete recording directory that resolves to that same key, so it does not require write access to a read-only ancestor. Setting `RUST_SREC_OUTPUT_ROOTS=/var/lib/rust-srec/output` gives the directory its own boundary; the longest matching configured prefix wins. Explicit boundaries are themselves probed and should name writable recording locations. See [output-root probes](../operations/storage.md#output-root-probes) for discovery limits.

## Resource Limits (Docker) {#resource-limits-docker}
| Variable | Description | Default |
|----------|-------------|---------|
| `CPU_LIMIT` | Maximum CPUs the container can use | `4` |
| `MEMORY_LIMIT` | Maximum memory the container can use | `4G` |
| `CPU_RESERVATION` | Reserved CPUs for the container | `1` |
| `MEMORY_RESERVATION` | Reserved memory for the container | `512M` |
