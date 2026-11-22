# Installation

## Choose a Deployment Method

| Method | Best for | Web interface | API |
|---|---|---|---|
| [Docker](./docker.md) | Most users and production hosts | `http://localhost:15275` | `http://localhost:12555/api` |
| [Desktop app](#desktop-app) | Recording on one personal computer | Built into the app | Internal to the app |
| Pre-built binary | Backend-only or custom deployments | Deploy the frontend separately | Backend default: `http://localhost:12555/api` |
| [systemd service](#systemd-service-linux) | Linux hosts running the backend without Docker | Deploy the frontend separately | `http://localhost:12555/api` |
| Source checkout with the example `.env` files | Development and contribution | `http://localhost:15275` | `http://localhost:12555/api` |

Docker maps the frontend's container port `80` and the backend's container port `8080` to the host ports shown above. Container ports are not addresses to open from the host.

## Docker (Recommended)

Docker packages the backend, frontend, and required runtime dependencies. Follow the [Docker deployment guide](./docker.md), then continue with [Make Your First Recording](./first-recording.md).

## Desktop App

The desktop app runs the backend and web interface together in one window on Windows, Linux, or macOS. Download its installer from the [GitHub Releases](https://github.com/hua0512/rust-srec/releases) page of a release that includes desktop builds. It needs no `.env` file: it generates its own `JWT_SECRET`, listens only on the local machine, and saves recordings to an `output` folder in its application data directory until you choose another **Output Folder**. Only one copy can run at a time, and it cannot share a database with a standalone backend. It signs you in automatically; if other people use the computer, turn on **Require password sign-in** under **Desktop → Backend & Security** and set your own password when asked.

## Pre-built Binaries

Download a package from [GitHub Releases](https://github.com/hua0512/rust-srec/releases). Packages are published for Linux, Windows, and macOS on the architectures listed with each release.

The `rust-srec` executable runs the backend. A complete browser-based installation also needs the frontend or the Docker deployment. Generate a unique `JWT_SECRET` of at least 32 characters before exposing the backend to another machine.

When the standalone backend creates a fresh database, it initializes the recording folder from `OUTPUT_DIR`, or `./output` if unset or blank. Relative paths are saved as absolute paths resolved from the startup working directory. Later starts preserve the saved setting, even if `OUTPUT_DIR` changes. Change an existing installation's recording folder in **Settings → Global → Output Folder**.

## systemd Service (Linux)

The repository includes `rust-srec/rust-srec.service`, a hardened unit that runs the pre-built backend binary as a system service. It supervises the backend only; deploy the frontend separately or use Docker for a complete browser installation.

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

The Docker Compose installer defaults `RUST_SREC_DIR` to `./rust-srec`, though the [Docker guide](./docker.md#installer) shows `/opt/rust-srec` as an example. Pick a different path if both deployments share a host and you install Docker into `/opt/rust-srec`.

Verify the service:

```bash
systemctl status rust-srec
journalctl -u rust-srec --since "5 min ago"
curl http://localhost:12555/api/health/live
```

### Set the Recording Directory

Fresh databases use `OUTPUT_DIR=/var/lib/rust-srec/output`. Existing databases keep their saved `output_folder` (including an older `/app/output` default from Docker); update **Settings → Global → Output Folder** to `/var/lib/rust-srec/output` or an existing, `rust-srec`-owned volume listed under `ReadWritePaths=`, and keep `RUST_SREC_OUTPUT_ROOTS` aligned so the output-root write gate stays healthy. For a collection past a few hundred GB, mount a separate volume under `ReadWritePaths=` rather than nesting it inside `/var/lib/rust-srec`.

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
| `RUST_SREC_SHUTDOWN_FORCE_RESERVE_SECS` | `2` |

Override any of them in `/etc/rust-srec/rust-srec.env`. `TimeoutStopSec=35` must stay larger than `RUST_SREC_SHUTDOWN_TIMEOUT_SECS`; raising one without the other lets systemd send `SIGKILL` while recordings are still being finalized.

### File Permissions and Hardening {#file-permissions}

`UMask=0027` writes recordings `-rw-r-----`, readable only by `rust-srec` and its group. Add readers (such as a media scanner) to the `rust-srec` group, or override `UMask=` in a drop-in. See [Security](../operations/security.md) for the unit's full sandboxing rules, `ProtectSystem=strict` details, and GPU transcoding overrides.

## Build from Source

### Requirements

- [rustup](https://rustup.rs/). The repository pins its toolchain in `rust-toolchain.toml`, and rustup installs and selects it automatically on the first `cargo` command. Any other Rust installation must be 1.95 or newer, the `rust-version` declared in the workspace `Cargo.toml`.
- Git and a C/C++ compiler. CMake and `protoc` are not needed.
- Node.js 26 and the pnpm version declared in `rust-srec/frontend/package.json` when running the frontend.
- Several gigabytes of free disk space for dependencies and build artifacts.

Install the native build tools for your operating system:

```bash
# Debian or Ubuntu
sudo apt-get install git build-essential

# Fedora or RHEL
sudo dnf install git gcc gcc-c++

# macOS
xcode-select --install
```

On Windows, install [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) with the **Desktop development with C++** workload, then Rust with the default MSVC host. Build with the default features; `--all-features` needs extra OpenSSL build tools on Windows.

For the desktop app, the tool-gated tests and other contributor setup, see [Development Environment](../development/setup.md).

### Build and Configure the Backend

```bash
git clone https://github.com/hua0512/rust-srec.git
cd rust-srec
cargo build --locked --release -p rust-srec
```

The backend loads `.env` using `dotenvy`, which searches the **current working directory** and parent directories upward — it does **not** search down into `rust-srec/`. Place `.env` where you plan to launch the binary:

```bash
# Option A: place .env in the repository root and run from the root
cp rust-srec/.env.example .env

# Option B: place .env inside rust-srec/ and run from rust-srec/
cd rust-srec && cp .env.example .env
```

Replace `JWT_SECRET` in `.env` with a random value of at least 32 characters.

Important backend settings:

| Variable | Purpose | Example-file value |
|---|---|---|
| `JWT_SECRET` | Signs access and refresh tokens; required | Replace the placeholder |
| `DATABASE_URL` | SQLite database location | `sqlite:./srec.db` |
| `API_BIND_ADDRESS` | Network interface to listen on | `0.0.0.0` |
| `API_PORT` | API port | `12555` |
| `OUTPUT_DIR` | Initial recording folder for a fresh standalone database. Does not override saved settings | `./output` |
| `RUST_LOG` | Log level | `info` |

Generate a secret with `openssl rand -hex 32`. In PowerShell:

```powershell
$bytes = New-Object Byte[] 32
[Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($bytes)
-join ($bytes | ForEach-Object { "{0:x2}" -f $_ })
```

Start the backend from the directory holding your `.env`:

```bash
# If .env is in the repository root:
./target/release/rust-srec

# If .env is inside rust-srec/:
cd rust-srec && ../target/release/rust-srec
```

### Run the Frontend for Development

```bash
cd rust-srec/frontend
cp .env.example .env
pnpm install --frozen-lockfile
pnpm dev
```

Replace `SESSION_SECRET` in the frontend `.env`. Its API settings already point to backend port `12555`; Vite serves the development UI on port `15275`.

## External Tools

The built-in Mesio download engine does not require Streamlink. Install external tools only for features you select:

- Install `ffmpeg` and make it available on `PATH` (or set `FFMPEG_PATH`) when using FFmpeg processors or an FFmpeg-based workflow outside the Docker image.
- Install `streamlink` and make it available on `PATH` when selecting the Streamlink download engine or extractor.
- Install [rclone](https://rclone.org/) and make it available on `PATH` (or set `RCLONE_PATH`) when using the `rclone` cloud-storage upload processor.
- Install [DanmakuFactory](https://github.com/hihkm/DanmakuFactory) and make it available on `PATH` (or set `DANMAKU_FACTORY_PATH`) when using the `danmaku_factory` XML-to-ASS subtitle conversion processor.
- Install [BaiduPCS-Go](https://github.com/qjfoidnh/BaiduPCS-Go) and make it available on `PATH` (or set `BAIDUPCS_PATH`) when using the `baidupcs` Baidu Netdisk upload processor.
- The Docker image contains its supported runtime tools; do not install them on the host for a standard Docker deployment.

## Next Step

Open the web interface for your deployment method and follow [Make Your First Recording](./first-recording.md). For Internet-facing or long-running hosts, complete the [Production Deployment](../operations/production.md) checklist first.
