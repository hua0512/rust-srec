<script setup>
import { withBase } from 'vitepress'
</script>

# Docker Deployment

Docker is the easiest and recommended way to deploy Rust-Srec.

## Prerequisites

- [Docker](https://docs.docker.com/get-docker/)
- [Docker Compose](https://docs.docker.com/compose/install/)

## Quick Start

### One-Line Install (Linux/macOS)

Run this command to automatically set up Rust-Srec:

```bash
curl -fsSL https://hua0512.github.io/rust-srec/docker-install.sh | bash
```

### One-Line Install (Windows PowerShell)

```powershell
irm https://hua0512.github.io/rust-srec/docker-install.ps1 | iex
```

The scripts will:
- Create the directory structure
- Download configuration files
- Generate secure secrets automatically
- Optionally start the application

### Manual Setup

1. Create a project directory:
   ```bash
   mkdir rust-srec && cd rust-srec
   mkdir -p data config output logs
   ```

2. Download the example configuration files:
   - <a :href="withBase('/docker-compose.example.yml')" download>docker-compose.example.yml</a>
   - <a :href="withBase('/env.example')" download=".env.example">.env.example</a>

3. Rename the files:
   ```bash
   # On Linux/macOS
   mv docker-compose.example.yml docker-compose.yml
   mv .env.example .env

   # On Windows
   rename docker-compose.example.yml docker-compose.yml
   rename .env.example .env
   ```

4. **Edit `.env`**: Make sure to set a secure `JWT_SECRET` and `SESSION_SECRET` (at least 32 characters).

::: tip Security Note
You can generate a secure random string using:
- **Linux/macOS**: `openssl rand -hex 32`
- **Windows (PowerShell)**: `$bytes = New-Object Byte[] 32; [Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($bytes); -join ($bytes | ForEach-Object { "{0:x2}" -f $_ })`
:::

5. Start the application:
   ```bash
   docker-compose up -d
   ```

::: warning Data Persistence
Ensure your `DATA_DIR` and `OUTPUT_DIR` are on a drive with sufficient space. Streaming recordings can consume disk space very quickly.
:::

## Configuration

The <a :href="withBase('/env.example')" download=".env.example">.env</a> file contains all the necessary environment variables.

| Variable | Description |
|----------|-------------|
| `JWT_SECRET` | Secret key for JWT signing (**Required**) |
| `SESSION_SECRET` | Secret for frontend session encryption (**Required**) |

::: tip Full Reference
For a complete list of all available environment variables and their descriptions, see the [Environment Variables Reference](./configuration.md#environment-variables).
:::

### docker-compose.yml

Our <a :href="withBase('/docker-compose.example.yml')" download>standard example</a> includes:
- **Automatic Restart**: `unless-stopped`
- **Healthchecks**: Ensures the frontend only starts after the backend is ready
- **Resource Limits**: Configurable CPU and memory limits
- **Log Rotation**: Prevents logs from filling up your disk

## Proxy Configuration

If you are behind a corporate proxy or in a region with restricted access, you can configure proxy settings for both the application and the download engines.

### 1. Configure Environment Variables

Add `HTTP_PROXY` and `HTTPS_PROXY` to your `.env` file:

```bash
# .env
HTTP_PROXY=http://your-proxy-host:port
HTTPS_PROXY=http://your-proxy-host:port
NO_PROXY=localhost,127.0.0.1,rust-srec
```

### 2. Update docker-compose.yml

Ensure these variables are passed to the `rust-srec` service:

```yaml
services:
  rust-srec:
    # ...
    environment:
      - HTTP_PROXY=${HTTP_PROXY:-}
      - HTTPS_PROXY=${HTTPS_PROXY:-}
      - NO_PROXY=${NO_PROXY:-}
    # ...
```

### 3. Enable in Application Settings

After starting the application, go to **Global Settings** > **Downloader** > **Proxy**:
1. Enable **Proxy**.
2. Check **Use System Proxy**.
3. Save settings.

This will instruct the application and its engines (FFmpeg, Streamlink, Mesio) to respect the environment variables you configured.

## Accessing the Application

- **Web Interface**: `http://localhost:[FRONTEND_PORT]` (Default: http://localhost:15275)
- **API reference**: `http://localhost:[API_PORT]/api/docs` (Default: http://localhost:12555/api/docs)

## Updating

To update to the latest version:
```bash
docker-compose pull
docker-compose up -d
```
