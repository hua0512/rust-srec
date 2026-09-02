<script setup>
import { withBase } from 'vitepress'
</script>

# Configuration

Rust-Srec uses a **4-layer configuration hierarchy** for flexible control. See [Configuration Layers](../concepts/configuration.md) for detailed architecture.

## Basic Configuration

### Adding Your First Streamer

1. Open the frontend at http://localhost:15275
2. Log in with default credentials:
   - **Username**: `admin`
   - **Password**: `admin123!`
3. Navigate to **Streamers** → **Add Streamer**
4. Enter:
   - **Name**: Display name
   - **URL**: Direct channel URL (e.g., `https://live.bilibili.com/<room-id>`)
   - **Platform**: Auto-detected from URL
5. Keep **Enable monitoring** on and click **Create streamer**

For a complete success check, follow [Make Your First Recording](./first-recording.md).

### Global Settings

Access via **Settings** → **Global Config**. The settings are organized into several categories:

#### File Configuration
| Setting | Description | Default |
|---------|-------------|---------|
| `record_danmu` | Enable danmaku (live chat) recording | `false` |
| `danmu_statistics` | How chat activity is summarised per session (see below) | defaults |
| `auto_thumbnail` | Automatically generate video thumbnails | `true` |
| `output_folder` | Base directory for recordings (supports templates) | `/app/output` |
| `output_filename_template` | Filename pattern for recorded files | (see below) |
| `output_file_format` | Default container format (mp4, flv, etc.) | `flv` |

#### Danmu Statistics

Every recording with `record_danmu` on gets a per-session chat summary: totals, an
activity timeline, the most active chatters, the most frequent words and — where the
platform reports them — gift rankings. `danmu_statistics` tunes that summary, and can
be set globally or overridden per platform, per template and per streamer. Any field
you leave out keeps its default, so `{"top_talkers": 200}` is a complete override.

| Field | Description | Default |
|-------|-------------|---------|
| `enabled` | Compute the summary at all. Turning it off still records the chat files; it only stops the summary, which stores viewer names, from being computed and saved. | `true` |
| `top_talkers` | Chatters and gift senders listed per session (1–500) | `100` |
| `top_words` | Frequent words listed per session (1–500) | `50` |
| `top_gifts` | Gift names listed per session (1–500) | `20` |
| `rate_bucket_secs` | Activity-timeline granularity in seconds. Very long streams are automatically coarsened, so the session page reads the width back rather than assuming it. | `10` |
| `talker_capacity` | Distinct chatters tracked (64–8192). While a stream has fewer than this, counts are exact; above it they become close estimates and the session page marks them with `≈`. | `2048` |
| `word_capacity` | Distinct words tracked (64–8192), same trade-off | `2048` |
| `gift_capacity` | Distinct gift names tracked | `256` |
| `extra_stop_words` | Words to exclude from the frequent-words chart, on top of the built-in list | none |

Out-of-range values are clamped rather than rejected, and a reported list is never
longer than what is tracked.

#### Resource Limits
| Setting | Description | Default |
|---------|-------------|---------|
| `min_segment_size` | Minimum size before a segment is kept | `1MB` |
| `max_download_duration_secs` | Max duration before splitting the recording | `0` (disabled) |
| `max_part_size` | Max size before splitting the recording | `8GB` |

#### Concurrency & Performance
| Setting | Description | Default |
|---------|-------------|---------|
| `max_concurrent_downloads` | Max simultaneous recording tasks | `6` |
| `max_concurrent_uploads` | Max simultaneous upload tasks | `3` |
| `max_cpu_jobs` | Max concurrent CPU-intensive tasks | `0` (Auto) |
| `max_io_jobs` | Max concurrent I/O-intensive tasks | `8` (0 = Auto) |
| `download_engine` | Engine used for recording (`ffmpeg`, `mesio`, etc.) | `mesio` |
| `queue_freshness_threshold` | When a recording has been waiting for a free slot longer than this, rust-srec re-checks the streamer to refresh stream URLs and headers before starting. Useful on platforms whose signed URLs expire within minutes. Set to `0` to refresh on every queue wait. | `60 Secs` |

#### Network & System
| Setting | Description | Default |
|---------|-------------|---------|
| `streamer_check_interval` | Interval between checking streamer status | `60 Secs` |
| `offline_check_interval` | Interval between checking offline status | `20 Secs` |
| `offline_detection_count` | Consecutive offline checks before confirming the streamer is offline. The same resolved count controls when consecutive download failures enter temporary cooldown. Download failures use a minimum threshold of `2`. | `3` |
| `retention_period` | Number of days to keep recordings in history | `30 Days` |
| `enable_proxy` | Route traffic through an intermediate server | `false` |

#### Pipeline Configuration
Rust-Srec features a powerful modular pipeline system where you can add custom steps (e.g., transcripts, notifications, custom scripts) at different stages:
- **Per-segment**: Runs for each recorded segment.
- **Paired Segment**: Runs for video/danmaku pairs.
- **Session Complete**: Runs when the entire recording session ends.

::: info Folder Organization
Set `output_folder` to `{streamer}/%Y-%m-%d` to organize recordings by streamer with date-based subfolders. The `output_filename_template` can then use `%H-%M-%S_{title}` for the filename itself.
:::

## Environment Variables

The following environment variables can be configured in your <a :href="withBase('/env.example')" download=".env.example">.env</a> file.

### General
| Variable | Description | Default |
|----------|-------------|---------|
| `TZ` | Container timezone | `UTC` |
| `VERSION` | Docker image version tag | `latest` |

### Paths
| Variable | Description | Default |
|----------|-------------|---------|
| `DATA_DIR` | Directory for application data | `./data` |
| `CONFIG_DIR` | Directory for platform configuration files | `./config` |
| `OUTPUT_DIR` | Directory where recordings are stored | `/app/output` |
| `LOG_DIR` | Directory for log files | `./logs` |

### Shutdown
| Variable | Description | Default |
|----------|-------------|---------|
| `RUST_SREC_SHUTDOWN_TIMEOUT_SECS` | Strict standalone-server shutdown deadline | `30` |
| `RUST_SREC_SHUTDOWN_FORCE_RESERVE_SECS` | Time reserved inside the deadline for forced process-tree containment; must be greater than zero and less than the total timeout | `2` |
| `RUST_SREC_CONTAINER_STOP_GRACE_PERIOD` | Docker Compose wait before external SIGKILL; keep longer than the backend deadline | `35s` |
| `RUST_SREC_RUNTIME_MARKER_PATH` | Dirty-generation marker retained after a forced or crashed runtime | Beside the SQLite database |

The deadline starts when the parent observes `SIGINT` or `SIGTERM`, including while startup admission or marker I/O is in progress, and covers the parent process exit as well as worker cleanup. The server first asks its isolated runtime to shut down gracefully; the runtime's own drain budget is derived from these same two values (the timeout minus the force reserve, less a small scheduling margin), so raising the timeout lengthens the phase that actually finalizes recordings. At the start of the force reserve, it terminates the contained process tree if the runtime is still active and exits unsuccessfully. Exit status `124` identifies hard-deadline expiry; `125` means the terminal process-tree termination request itself failed. A worker-local fatal failure fails closed instead of starting an unbounded graceful drain. A retained marker means startup recovery may be required; later clean runs do not erase that earlier recovery debt. The marker does not by itself reconstruct an artifact that was interrupted before it reached SQLite. Remove it only while the backend is stopped and after the interrupted artifacts have been reconciled. Recording engines clamp their own graceful-stop wait to whatever remains of this budget, so a per-engine stop timeout that is longer than the shutdown timeout no longer causes the engine child to be killed mid-finalization. A shutdown that overruns its grace period but still finalizes everything exits cleanly; only work that could not be contained is reported as a crash.

### Network
| Variable | Description | Default |
|----------|-------------|---------|
| `API_BIND_ADDRESS` | IP address the backend API binds to | `0.0.0.0` |
| `API_PORT` | External port for the backend API | `12555` |
| `FRONTEND_PORT` | External port for the web interface | `15275` |
| `BACKEND_URL` | Internal URL for the frontend to reach the backend | `http://rust-srec:8080` |
| `HTTP_PROXY` | HTTP proxy server URL | - |
| `HTTPS_PROXY` | HTTPS proxy server URL | - |
| `NO_PROXY` | Comma-separated list of hosts to bypass proxy | - |

### Security & Auth
| Variable | Description | Default |
|----------|-------------|---------|
| `JWT_SECRET` | Secret key for JWT signing (**Required** unless using the local-only opt-out below) | - |
| `AUTH_DISABLED` | Disable backend authentication for loopback-only local development | `false` |
| `API_CORS_ORIGINS` | Comma-separated exact browser origins (`scheme://host[:port]`) allowed to call the API cross-origin while authentication is disabled | Local dev server and desktop webview origins |
| `JWT_ISSUER` | JWT issuer identifier | `rust-srec` |
| `JWT_AUDIENCE` | JWT audience identifier | `rust-srec-api` |
| `SESSION_SECRET` | Frontend session encryption secret (**Required**, min 32 chars) | - |
| `COOKIE_SECURE` | Set to `true` to force HTTPS-only cookies | (auto) |
| `MIN_PASSWORD_LENGTH` | Minimum length for user passwords | `8` |

The backend refuses to start without a non-empty `JWT_SECRET`. For local development only, authentication can be disabled by setting both `AUTH_DISABLED=true` and `API_BIND_ADDRESS=127.0.0.1` (or `::1`). The backend rejects this opt-out for wildcard, hostname, and non-loopback bind addresses.

While authentication is disabled, only the origins in `API_CORS_ORIGINS` may call the API from a browser; the default list covers `http://localhost:15275`, `http://127.0.0.1:15275`, `tauri://localhost`, and `http://tauri.localhost`. Set the variable to override it — entries must be exact origins with no trailing path, and malformed entries are skipped with a warning at startup. With authentication enabled the variable is ignored and any origin may send requests, because every protected route still requires a bearer token.

### Token Expiration
| Variable | Description | Default |
|----------|-------------|---------|
| `ACCESS_TOKEN_EXPIRATION_SECS` | JWT access token lifetime | `3600` (1h) |
| `REFRESH_TOKEN_EXPIRATION_SECS` | JWT refresh token lifetime | `604800` (7d) |

### Browser Notifications (Web Push / VAPID)
| Variable | Description | Default |
|----------|-------------|---------|
| `WEB_PUSH_VAPID_PUBLIC_KEY` | VAPID public key (base64url, unpadded). Leave empty/unset to disable. | - |
| `WEB_PUSH_VAPID_PRIVATE_KEY` | VAPID private key (base64url, unpadded). Leave empty/unset to disable. | - |
| `WEB_PUSH_VAPID_SUBJECT` | VAPID subject (e.g. `mailto:admin@localhost`) | `mailto:admin@localhost` |

### Backend Service
| Variable | Description | Default |
|----------|-------------|---------|
| `RUST_LOG` | Logging level (`trace`, `debug`, `info`, `warn`, `error`) | `info` |
| `DATABASE_URL` | SQL database connection string | `sqlite:///app/data/rust-srec.db` |
| `RUST_SREC_LOCALE` | Locale for backend-emitted notification strings. Affects every notification event — stream online/offline, download lifecycle, segments, pipeline jobs, system alerts, credential events. Supported: `en`, `zh-CN`. | `en` |
| `RUST_SREC_OUTPUT_ROOTS` | Comma-separated list of **absolute** paths to treat as output-root boundaries for the write gate. If unset, the gate uses a heuristic that takes the first **two named components** of each resolved output path (e.g. `/rec/huya` for `/rec/huya/X/20260415`, `/home/user` for `/home/user/recordings/X/20260415`). Two named components is the smallest safe default — it avoids accidentally sharing a gate key across unrelated users in `/home/...` layouts. For a single-mount `/rec`-style layout where you want one gate key per mount (and therefore one aggregated notification on failure instead of one per platform), set this explicitly: `RUST_SREC_OUTPUT_ROOTS=/rec`. | - |

### Resource Limits (Docker)
| Variable | Description | Default |
|----------|-------------|---------|
| `CPU_LIMIT` | Maximum CPUs the container can use | `4` |
| `MEMORY_LIMIT` | Maximum memory the container can use | `4G` |
| `CPU_RESERVATION` | Reserved CPUs for the container | `1` |
| `MEMORY_RESERVATION` | Reserved memory for the container | `512M` |

## Filename Template Variables

Rust-Srec supports two types of placeholders in `output_folder` and `output_filename_template`.

### Curly Brace Variables
These are replaced with streamer or session specific metadata.

| Variable | Description |
|----------|-------------|
| `{streamer}` | Streamer display name |
| `{title}` | Current stream title |
| `{platform}` | Platform name (e.g., bilibili) |
| `{session_id}` | Unique ID for the recording session (only in `output_folder`) |

### Percent Placeholders (FFmpeg Style)
These are replaced with date, time, or sequence information.

| Variable | Description |
|----------|-------------|
| `%Y` | Year (YYYY) |
| `%m` | Month (01-12) |
| `%d` | Day (01-31) |
| `%H` | Hour (00-23) |
| `%M` | Minute (00-59) |
| `%S` | Second (00-59) |
| `%i` | Sequence number for split parts |
| `%t` | Unix timestamp |
| `%%` | Literal percent sign |

Example: `{streamer}/%Y-%m-%d/%H-%M-%S_{title}`

### Pipeline Destination Placeholders

Pipeline destination fields such as rclone `destination_root` and copy/move
`destination` support `{platform}`, `{streamer}`, `{title}`, `{streamer_id}`,
`{session_id}`, and the same `%Y`, `%m`, `%d`, `%H`, `%M`, `%S`, `%t`, and `%%`
time tokens. Time tokens render in the server's local time zone.

Rclone expands time tokens with the job creation time by default. Set
`time_anchor` to `session_start` to keep every segment from one live session in
the folder for the session's start date, even when the stream crosses midnight.
Copy/move preserves its historical execution-time expansion when `time_anchor`
is omitted; set it to `job_created` or `session_start` when deterministic
anchoring is needed.

When anchoring by session start, keep `%Y%m%d-%H%M%S` or `%t` in the filename
template. If multiple sessions send the same basename into one destination
folder, rclone and filesystem copy/move operations can overwrite or skip files
depending on the operation and arguments.
