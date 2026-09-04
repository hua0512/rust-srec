<script setup>
import { withBase } from 'vitepress'
</script>

# Docker Deployment

The standard deployment runs a backend and frontend on one Docker host with persistent bind mounts.

## Prerequisites

- [Docker Engine or Docker Desktop](https://docs.docker.com/get-docker/)
- [Docker Compose v2](https://docs.docker.com/compose/install/), invoked as `docker compose`
- Persistent storage sized for recordings

## Installer

Linux and macOS:

```bash
curl -fsSL https://docs.srec.rs/install.sh | bash
```

Windows PowerShell:

```powershell
irm https://docs.srec.rs/install.ps1 | iex
```

The installer downloads Compose and environment files, generates unique secrets, detects optional NVIDIA support, and offers to start the services. Both entry points pick a localized installer from `SREC_LANG` or the system locale; set `SREC_LANG` explicitly to override it.

::: warning Review Remote Scripts
Piping a remote script to a shell executes the current network response. Environments with change control should download the script, verify its contents and checksum, then run the reviewed local copy or use the manual setup below.
:::

Installer settings:

| Variable | Purpose | Default |
|---|---|---|
| `SREC_LANG` | `en` or `zh` | Detected from locale |
| `RUST_SREC_DIR` | Installation directory | `./rust-srec` |
| `VERSION` | Image tag | `latest` |

For a reviewed release, set an explicit version. Example for Linux/macOS:

```bash
curl -fsSL https://docs.srec.rs/install.sh | RUST_SREC_DIR=/opt/rust-srec VERSION=v0.5.1 bash
```

::: warning Path Collision with the systemd Service
The [systemd service](./installation.md#systemd-service-linux) installs the backend binary at `/opt/rust-srec/rust-srec`. Choose a different `RUST_SREC_DIR` if both deployments share a host.
:::

## Manual Setup

1. Create a directory and download both files:

   - <a :href="withBase('/docker-compose.example.yml')" download>docker-compose.example.yml</a>
   - <a :href="withBase('/env.example')" download>.env.example</a>

2. Rename them to `docker-compose.yml` and `.env`.
3. Generate two different secrets and set `JWT_SECRET` and `SESSION_SECRET` in `.env`. Empty values intentionally make Compose refuse to start.
4. Review paths, timezone, ports, and `VERSION`.
5. Start and verify:

```bash
docker compose up -d
docker compose ps
curl http://localhost:12555/api/health/live
```

Generate a secret with `openssl rand -hex 32`. Where openssl is unavailable, use `python3 -c 'import secrets; print(secrets.token_hex(32))'`. On PowerShell:

```powershell
$bytes = New-Object Byte[] 32
[Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($bytes)
-join ($bytes | ForEach-Object { "{0:x2}" -f $_ })
```

## Default Layout

| Host | Container | Purpose |
|---|---|---|
| `./data` | `/app/data` | SQLite and application data |
| `./config` | `/app/config` | Platform configuration |
| `./output` | `/app/output` | Recordings and pipeline output |
| `./logs` | `/app/logs` | Application logs |
| `12555` | Backend `8080` | API and Swagger |
| `15275` | Frontend `80` | Web interface |

Use absolute host paths for a long-running deployment. The example enables `unless-stopped`, backend health checks, frontend startup ordering, and container log rotation.

## Access and First Login

- Web interface: `http://localhost:15275`
- Swagger UI: `http://localhost:12555/api/docs`
- Initial account: `admin` / `admin123!`

The first login requires a password change. Continue with [Make Your First Recording](./first-recording.md).

## Optional Configuration

### Proxy

Set `HTTP_PROXY`, `HTTPS_PROXY`, and `NO_PROXY` in `.env`; the example already passes them to the backend. Then enable **Global Settings > Downloader > Proxy > Use System Proxy**. Include `localhost,127.0.0.1,rust-srec` in `NO_PROXY`.

### Browser Push

Generate VAPID keys, place them in `.env`, and restart the backend. Web Push requires HTTPS outside localhost. See [Notification System](../concepts/notifications.md#web-push).

### NVIDIA GPU

Install the host driver and [NVIDIA Container Toolkit](https://docs.nvidia.com/datacenter/cloud-native/container-toolkit/latest/install-guide.html), then download <a :href="withBase('/docker-compose.gpu.yml')" download>docker-compose.gpu.yml</a> and start with:

```bash
docker compose -f docker-compose.yml -f docker-compose.gpu.yml up -d
docker exec rust-srec nvidia-smi
```

GPU access only enables compatible processors; select an NVENC processor in the pipeline and monitor the GPU component on **System Health**.

## Storage Cleanup

Delete files inside the mounted output directory; do not rename, replace, or move the host directory that is itself bind-mounted while the container is running. File managers that implement deletion by moving the root to trash can leave the active container attached to an orphaned directory.

If System Health reports output-root `not_found`, verify the host directory and restart the container to recreate the mount namespace. For `storage_full`, free capacity without replacing the directory; the gate can recover on a later probe. See [Storage and Capacity](../operations/storage.md) and the [FAQ](./faq.md#i-freed-disk-space-but-recording-did-not-resume).

## Stop, Update, and Remove

```bash
# Stop while keeping data
docker compose stop

# Start again
docker compose up -d
```

Use [Upgrade and Rollback](../operations/upgrading.md) before changing `VERSION`. `docker compose down` removes containers and the network but leaves bind-mounted host data; verify exact paths before deleting any host directory.

For an Internet-facing host, complete [Production Deployment](../operations/production.md) instead of publishing the example ports directly.
