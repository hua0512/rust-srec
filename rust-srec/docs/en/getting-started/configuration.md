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
   - **URL**: Stream URL (e.g., `https://www.bilibili.com/xxxx`)
   - **Platform**: Auto-detected from URL
5. Click **Save**

### Global Settings

Access via **Settings** → **Global Config**. The settings are organized into several categories:

#### File Configuration
| Setting | Description | Default |
|---------|-------------|---------|
| `record_danmu` | Enable danmaku (live chat) recording | `false` |
| `auto_thumbnail` | Automatically generate video thumbnails | `true` |
| `output_folder` | Base directory for recordings (supports templates) | `/app/output` |
| `output_filename_template` | Filename pattern for recorded files | (see below) |
| `output_file_format` | Default container format (mp4, flv, etc.) | `flv` |

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

#### Network & System
| Setting | Description | Default |
|---------|-------------|---------|
| `streamer_check_interval` | Interval between checking streamer status | `60 Secs` |
| `offline_check_interval` | Interval between checking offline status | `20 Secs` |
| `offline_detection_count` | Retries before marking streamer as offline | `3` |
| `retention_period` | Number of days to keep recordings in history | `30 Days` |
| `session_gap_time_secs` | Time to wait before considering a session complete | `1 Hour` |
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

The following environment variables can be configured in your [`.env`](/.env.example) file.

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

### Network
| Variable | Description | Default |
|----------|-------------|---------|
| `API_PORT` | External port for the backend API | `12555` |
| `FRONTEND_PORT` | External port for the web interface | `15275` |
| `BACKEND_URL` | Internal URL for the frontend to reach the backend | `http://rust-srec:8080` |
| `HTTP_PROXY` | HTTP proxy server URL | - |
| `HTTPS_PROXY` | HTTPS proxy server URL | - |
| `NO_PROXY` | Comma-separated list of hosts to bypass proxy | - |

### Security & Auth
| Variable | Description | Default |
|----------|-------------|---------|
| `JWT_SECRET` | Secret key for JWT signing (**Required**) | - |
| `JWT_ISSUER` | JWT issuer identifier | `rust-srec` |
| `JWT_AUDIENCE` | JWT audience identifier | `rust-srec-api` |
| `SESSION_SECRET` | Frontend session encryption secret (**Required**, min 32 chars) | - |
| `COOKIE_SECURE` | Set to `true` to force HTTPS-only cookies | (auto) |
| `MIN_PASSWORD_LENGTH` | Minimum length for user passwords | `8` |

### Token Expiration
| Variable | Description | Default |
|----------|-------------|---------|
| `ACCESS_TOKEN_EXPIRATION_SECS` | JWT access token lifetime | `3600` (1h) |
| `REFRESH_TOKEN_EXPIRATION_SECS` | JWT refresh token lifetime | `604800` (7d) |

### Backend Service
| Variable | Description | Default |
|----------|-------------|---------|
| `RUST_LOG` | Logging level (`trace`, `debug`, `info`, `warn`, `error`) | `info` |
| `DATABASE_URL` | SQL database connection string | `sqlite:///app/data/rust-srec.db` |

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
