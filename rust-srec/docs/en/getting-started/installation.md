# Installation

## Choose a Deployment Method

| Method | Best for | Web interface | API |
|---|---|---|---|
| [Docker](./docker.md) | Most users and production hosts | `http://localhost:15275` | `http://localhost:12555/api` |
| Pre-built binary | Backend-only or custom deployments | Deploy the frontend separately | Backend default: `http://localhost:12555/api` |
| [systemd service](#systemd-service-linux) | Linux hosts running the backend without Docker | Deploy the frontend separately | `http://localhost:12555/api` |
| Source checkout with the example `.env` files | Development and contribution | `http://localhost:15275` | `http://localhost:8080/api` |

Docker maps the frontend's container port `80` and the backend's container port `8080` to the host ports shown above. Container ports are not addresses to open from the host.

## Docker (Recommended)

Docker packages the backend, frontend, and required runtime dependencies. Follow the [Docker deployment guide](./docker.md), then continue with [Make Your First Recording](./first-recording.md).

## Pre-built Binaries

Download a package from [GitHub Releases](https://github.com/hua0512/rust-srec/releases). Packages are published for Linux, Windows, and macOS on the architectures listed with each release.

The `rust-srec` executable runs the backend. A complete browser-based installation also needs the frontend or the Docker deployment. Generate a unique `JWT_SECRET` of at least 32 characters before exposing the backend to another machine.

## systemd Service (Linux)

The repository ships `rust-srec/rust-srec.service`, a hardened unit that runs the pre-built backend binary as a system service. It supervises the backend only; deploy the frontend separately or use Docker for a complete browser installation.

### Install

Run as root from the directory holding the downloaded `rust-srec` binary and `rust-srec.service`:

```bash
useradd --system --home-dir /var/lib/rust-srec --shell /usr/sbin/nologin rust-srec
install -D -m 0755 rust-srec /opt/rust-srec/rust-srec
install -d -m 0750 -o root -g rust-srec /etc/rust-srec
umask 027 && printf 'JWT_SECRET=%s\n' "$(openssl rand -hex 32)" \
  > /etc/rust-srec/rust-srec.env
chown root:rust-srec /etc/rust-srec/rust-srec.env
install -D -m 0644 rust-srec.service /etc/systemd/system/rust-srec.service
```

On a reinstall, where `/var/lib/rust-srec` or `/var/log/rust-srec` survive from an earlier install, take ownership before the first start:

```bash
[ -d /var/lib/rust-srec ] && chown -R rust-srec:rust-srec /var/lib/rust-srec
[ -d /var/log/rust-srec ] && chown -R rust-srec:rust-srec /var/log/rust-srec
```

Skipping this leaves systemd to fix the ownership itself during unit start, and on a large recordings tree that can outlast the start timeout and fail the start.

Then start the service:

```bash
systemctl daemon-reload && systemctl enable --now rust-srec
```

The account's home directory has to be `/var/lib/rust-srec` and has to be writable — rclone and BaiduPCS-Go rewrite their session files there. The install directory under `/opt` cannot serve as a home; the unit's hardening keeps it read-only.

`StateDirectory=` and `LogsDirectory=` create `/var/lib/rust-srec`, `/var/lib/rust-srec/output`, and `/var/log/rust-srec` on each start, so a fresh host needs nothing pre-created.

`/etc/rust-srec/rust-srec.env` is loaded through `EnvironmentFile=`. The service refuses to start without `JWT_SECRET`, so that file is mandatory in practice. Web Push (VAPID) keys and any `*_PATH` override for ffmpeg, rclone, streamlink, or DanmakuFactory belong there too. Every assignment in it overrides the unit's own `Environment=` lines.

`/opt/rust-srec` is also the directory the Docker Compose installer defaults to (`RUST_SREC_DIR`). Pick a different path for the binary if both deployments share a host.

Verify the service:

```bash
systemctl status rust-srec
journalctl -u rust-srec --since "5 min ago"
curl http://localhost:12555/api/health/live
```

### Set the Recording Directory

::: warning Recordings fail until the output folder is set
The database ships with `output_folder` set to `/app/output`, a path that belongs to the Docker image and that `ProtectSystem=strict` neither provides nor makes writable. No environment variable overrides it. Until **Settings → Global → Output Folder** names `/var/lib/rust-srec/output` or a volume listed under `ReadWritePaths=`, the service starts normally and every recording fails.
:::

Keep `RUST_SREC_OUTPUT_ROOTS` in step with that value, or the output-root write gate reports the service degraded. The unit ships both pointing at `/var/lib/rust-srec/output`.

Give a collection that grows past a few hundred GB its own volume and list it under `ReadWritePaths=` rather than nesting it inside `/var/lib/rust-srec`. Every `ReadWritePaths=` entry must already exist — `ProtectSystem=strict` fails the unit with `226/NAMESPACE` on a missing path — and systemd does not chown them, so create them owned by `rust-srec`.

### What the Unit Sets

| Variable | Value |
|---|---|
| `DATABASE_URL` | `sqlite:///var/lib/rust-srec/rust-srec.db` |
| `LOG_DIR` | `/var/log/rust-srec` |
| `OUTPUT_DIR` | `/var/lib/rust-srec/output` |
| `RUST_SREC_OUTPUT_ROOTS` | `/var/lib/rust-srec/output` |
| `API_BIND_ADDRESS` | `0.0.0.0` |
| `API_PORT` | `12555` |
| `RUST_LOG` | `info` |
| `RUST_SREC_SHUTDOWN_TIMEOUT_SECS` | `30` |

Override any of them in `/etc/rust-srec/rust-srec.env`. `TimeoutStopSec=35` must stay larger than `RUST_SREC_SHUTDOWN_TIMEOUT_SECS`; raising one without the other lets systemd send `SIGKILL` while recordings are still being finalized.

### File Permissions

`UMask=0027` writes recordings `-rw-r-----`, readable only by `rust-srec` and its group. Add any account that has to read them — a file server, a media scanner — to the `rust-srec` group, or lower `UMask=` in a drop-in. See [Security](../operations/security.md) for the rest of the unit's hardening and its effect on GPU transcoding.

## Build from Source

### Requirements

- Rust 1.95 or newer on the stable channel. This is the `rust-version` declared in the workspace `Cargo.toml`; Cargo refuses to build with anything older.
- Git, CMake 3.12 or newer, and a C/C++ compiler.
- Node.js 26 and the pnpm version declared in `rust-srec/frontend/package.json` when running the frontend.
- At least 2 GB of free disk space for dependencies and build artifacts.

Install the native build tools for your operating system:

```bash
# Debian or Ubuntu
sudo apt-get install git cmake build-essential

# Fedora or RHEL
sudo dnf install git cmake gcc g++

# macOS
xcode-select --install
brew install cmake
```

On Windows, install [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) with the C++ workload and [CMake](https://cmake.org/download/). See the [aws-lc-rs requirements](https://aws.github.io/aws-lc-rs/requirements/index.html) if native compilation fails.

### Build and Configure the Backend

```bash
git clone https://github.com/hua0512/rust-srec.git
cd rust-srec
cargo build --locked --release -p rust-srec

cd rust-srec
cp .env.example .env
```

Replace `JWT_SECRET` in `.env` with a random value of at least 32 characters. The checked-in example sets `API_PORT=8080`; without an `.env` file, the backend defaults to port `12555`.

Important backend settings:

| Variable | Purpose | Example-file value |
|---|---|---|
| `JWT_SECRET` | Signs access and refresh tokens; required | Replace the placeholder |
| `DATABASE_URL` | SQLite database location | `sqlite:./srec.db` |
| `API_BIND_ADDRESS` | Network interface to listen on | `0.0.0.0` |
| `API_PORT` | API port | `8080` |
| `OUTPUT_DIR` | Output root the write gate and disk-space probe watch; it does not set where recordings are written | `./output` |
| `RUST_LOG` | Log level | `info` |

Generate a secret with `openssl rand -hex 32`. In PowerShell:

```powershell
$bytes = New-Object Byte[] 32
[Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($bytes)
-join ($bytes | ForEach-Object { "{0:x2}" -f $_ })
```

Start the backend from the repository root:

```bash
./target/release/rust-srec
```

### Run the Frontend for Development

```bash
cd rust-srec/frontend
cp .env.example .env
pnpm install --frozen-lockfile
pnpm dev
```

Replace `SESSION_SECRET` in the frontend `.env`. Its API settings already point to the example backend port `8080`; Vite serves the development UI on port `15275`.

## External Tools

The built-in Mesio download engine does not require Streamlink. Install external tools only for features you select:

- Install `ffmpeg` and make it available on `PATH` when using FFmpeg processors or an FFmpeg-based workflow outside the Docker image.
- Install `streamlink` and make it available on `PATH` when selecting the Streamlink download engine.
- Install [BaiduPCS-Go](https://github.com/qjfoidnh/BaiduPCS-Go) and make it available on `PATH` (or set `BAIDUPCS_PATH`) when using the `baidupcs` Baidu Netdisk upload processor.
- The Docker image contains its supported runtime tools; do not install them on the host for a standard Docker deployment.

## Next Step

Open the web interface for your deployment method and follow [Make Your First Recording](./first-recording.md). For Internet-facing or long-running hosts, complete the [Production Deployment](../operations/production.md) checklist first.

