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
curl -fsSL https://docs.srec.rs/docker-install.sh | bash
```

### One-Line Install (Windows PowerShell)

```powershell
irm https://docs.srec.rs/install.ps1 | iex
```

The scripts will:
- Download configuration files
- Generate secure secrets automatically
- Optionally start the application

::: tip Customizing Installation
The script auto-detects your system language. You can also customize the installation using environment variables:

**Linux/macOS:**
```bash
# Install dev version to custom directory
RUST_SREC_DIR=/opt/rust-srec VERSION=dev curl -fsSL https://docs.srec.rs/docker-install.sh | bash
```

**Windows PowerShell:**
```powershell
# Install dev version to custom directory
$env:RUST_SREC_DIR = "C:\rust-srec"; $env:VERSION = "dev"; irm https://docs.srec.rs/install.ps1 | iex
```

| Variable | Description | Default |
|----------|-------------|---------|
| `SREC_LANG` | Language (`zh` or `en`) | Auto-detect |
| `RUST_SREC_DIR` | Installation directory | `./rust-srec` |
| `VERSION` | Docker image tag (`latest` or `dev`) | `latest` |
:::

### Manual Setup

1. Create a project directory:
   ```bash
   mkdir rust-srec && cd rust-srec
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

### Browser Notifications (Web Push)

To enable browser push notifications, generate VAPID keys and set them in `.env`:

```bash
docker run --rm ghcr.io/hua0512/rust-srec:latest /app/rust-srec-vapid
# or: npx --yes web-push generate-vapid-keys
```

| Variable | Description |
|----------|-------------|
| `WEB_PUSH_VAPID_PUBLIC_KEY` | VAPID public key (base64url, unpadded) |
| `WEB_PUSH_VAPID_PRIVATE_KEY` | VAPID private key (base64url, unpadded) |
| `WEB_PUSH_VAPID_SUBJECT` | VAPID subject (e.g. `mailto:admin@localhost`) |

::: tip Note
Web Push requires HTTPS (or localhost).
:::

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

## GPU Hardware Acceleration (NVIDIA)

If you have an NVIDIA GPU, you can enable hardware-accelerated video transcoding (NVENC/NVDEC) to dramatically reduce CPU usage and speed up remuxing/transcoding.

### Prerequisites

1. **NVIDIA GPU drivers** installed on the host machine
2. **NVIDIA Container Toolkit** installed on the host — this allows Docker containers to access the GPU:
   - [Installation Guide](https://docs.nvidia.com/datacenter/cloud-native/container-toolkit/install-guide.html)
   - Quick install (Ubuntu/Debian):
     ```bash
     curl -fsSL https://nvidia.github.io/libnvidia-container/gpgkey | sudo gpg --dearmor -o /usr/share/keyrings/nvidia-container-toolkit-keyring.gpg
     curl -s -L https://nvidia.github.io/libnvidia-container/stable/deb/nvidia-container-toolkit.list | \
       sed 's#deb https://#deb [signed-by=/usr/share/keyrings/nvidia-container-toolkit-keyring.gpg] https://#g' | \
       sudo tee /etc/apt/sources.list.d/nvidia-container-toolkit.list
     sudo apt-get update && sudo apt-get install -y nvidia-container-toolkit
     sudo nvidia-ctk runtime configure --runtime=docker
     sudo systemctl restart docker
     ```

### Enable GPU in docker-compose

Download the GPU compose override file alongside your `docker-compose.yml`:

- <a :href="withBase('/docker-compose.gpu.yml')" download>docker-compose.gpu.yml</a>

Then start with both files:

```bash
docker compose -f docker-compose.yml -f docker-compose.gpu.yml up -d
```

Or set `COMPOSE_FILE` in your `.env` so `docker compose up -d` picks it up automatically:

```bash
echo "COMPOSE_FILE=docker-compose.yml:docker-compose.gpu.yml" >> .env
```

The override file adds the NVIDIA device reservation:

```yaml
services:
  rust-srec:
    deploy:
      resources:
        reservations:
          devices:
            - driver: nvidia
              count: 1
              capabilities: [gpu, video]
```

::: tip Automatic Setup
If you use our [one-line install script](#one-line-install-linux-macos), it will automatically detect your NVIDIA GPU and offer to enable this for you.
:::

### Verify GPU Access

After starting the container, verify the GPU is accessible:

```bash
docker exec rust-srec nvidia-smi
```

You should see your GPU model and driver version. If you get an error, the NVIDIA Container Toolkit is not properly configured.

### Enable in Application

Once the GPU is accessible to the container, go to your recording preset settings and enable **Hardware Acceleration** with the `cuda` device to use NVENC for encoding.

### Troubleshooting

| Symptom | Cause | Fix |
|---------|-------|-----|
| `Cannot load libnvcuvid.so.1` in ffmpeg logs | Container cannot access GPU drivers | Install NVIDIA Container Toolkit and restart Docker |
| `nvidia-smi` not found inside container | Container Toolkit not configured | Run `sudo nvidia-ctk runtime configure --runtime=docker && sudo systemctl restart docker` |
| High CPU usage despite GPU enabled | Wrong encoder selected | Ensure the preset uses `h264_nvenc` or `hevc_nvenc`, not software encoders |

## Accessing the Application

- **Web Interface**: `http://localhost:[FRONTEND_PORT]` (Default: http://localhost:15275)
- **API reference**: `http://localhost:[API_PORT]/api/docs` (Default: http://localhost:12555/api/docs)

## Updating

To update to the latest version:
```bash
docker-compose pull
docker-compose up -d
```
