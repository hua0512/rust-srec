# Installation

## Choose a Deployment Method

| Method | Best for | Web interface | API |
|---|---|---|---|
| [Docker](./docker.md) | Most users and production hosts | `http://localhost:15275` | `http://localhost:12555/api` |
| Pre-built binary | Backend-only or custom deployments | Deploy the frontend separately | Backend default: `http://localhost:12555/api` |
| Source checkout with the example `.env` files | Development and contribution | `http://localhost:15275` | `http://localhost:8080/api` |

Docker maps the frontend's container port `80` and the backend's container port `8080` to the host ports shown above. Container ports are not addresses to open from the host.

## Docker (Recommended)

Docker packages the backend, frontend, and required runtime dependencies. Follow the [Docker deployment guide](./docker.md), then continue with [Make Your First Recording](./first-recording.md).

## Pre-built Binaries

Download a package from [GitHub Releases](https://github.com/hua0512/rust-srec/releases). Packages are published for Linux, Windows, and macOS on the architectures listed with each release.

The `rust-srec` executable runs the backend. A complete browser-based installation also needs the frontend or the Docker deployment. Generate a unique `JWT_SECRET` of at least 32 characters before exposing the backend to another machine.

## Build from Source

### Requirements

- Rust 1.95 or newer on the stable channel. This is the `rust-version` declared in the workspace `Cargo.toml`; Cargo refuses to build with anything older.
- Git, CMake 3.12 or newer, and a C/C++ compiler.
- Node.js 24 and the pnpm version declared in `rust-srec/frontend/package.json` when running the frontend.
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
| `OUTPUT_DIR` | Recording output directory | `./output` |
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
- The Docker image contains its supported runtime tools; do not install them on the host for a standard Docker deployment.

## Next Step

Open the web interface for your deployment method and follow [Make Your First Recording](./first-recording.md). For Internet-facing or long-running hosts, complete the [Production Deployment](../operations/production.md) checklist first.

