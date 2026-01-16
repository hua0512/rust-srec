# Installation

## Docker (Recommended)

Docker is the easiest and recommended way to run Rust-Srec.

See the [Docker deployment guide](./docker.md) for complete setup instructions.

## Pre-built Binaries

Download pre-built binaries from the [GitHub Releases](https://github.com/hua0512/rust-srec/releases) page.

Available platforms:
- Linux (x86_64, aarch64)
- Windows (x86_64)
- macOS (x86_64, aarch64)

## From Source

### Environment Requirements

Before building from source, ensure your system meets the following requirements:

#### Rust Toolchain

- **Minimum Version**: Rust 1.83.0 (2024 edition)
- **Channel**: Stable
- **Installation**: Use [rustup](https://rustup.rs/) for easy installation and management

```bash
# Install Rust using rustup
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Or on Windows, download and run rustup-init.exe from https://rustup.rs/

# Verify installation
rustc --version
cargo --version
```

#### System Requirements

- **RAM**: Minimum 2GB available (4GB+ recommended for faster builds)
- **Disk Space**: At least 2GB free space for dependencies and build artifacts
- **Network**: Internet connection required for downloading dependencies

### Build Prerequisites

#### Required Tools

- **Git**: Version control system
  - Linux: `sudo apt-get install git` (Debian/Ubuntu) or `sudo dnf install git` (Fedora/RHEL)
  - Windows: [Download Git](https://git-scm.com/download/win)
  - macOS: Included with Xcode Command Line Tools

- **CMake**: Required for building aws-lc-rs (minimum version 3.12)
  - Versions 3.12 or later are required
  
- **C/C++ Compiler**: 
  - **Linux**: GCC 7.1+ or Clang 5.0+
  - **Windows**: MSVC (Visual Studio 2017 or later / Visual Studio Build Tools)
  - **macOS**: Xcode Command Line Tools (macOS 10.13+ SDK)

#### aws-lc-rs Requirements

Rust-Srec uses [aws-lc-rs](https://github.com/aws/aws-lc-rs) for cryptography.

For complete platform-specific requirements, see the [official aws-lc-rs documentation](https://aws.github.io/aws-lc-rs/requirements/index.html).

**Linux (Debian/Ubuntu):**
```bash
sudo apt-get install cmake build-essential
```

**Linux (Fedora/RHEL):**
```bash
sudo dnf install cmake gcc g++
```

**macOS:**
```bash
xcode-select --install
brew install cmake
```

**Windows:**
- Install [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/)
- Install [CMake](https://cmake.org/download/)
- Ensure both are in your PATH

### Build

```bash
# Clone the repository
git clone https://github.com/hua0512/rust-srec.git
cd rust-srec

# Build release binary
cargo build --release -p rust-srec

# Binary will be at target/release/rust-srec
```

### Environment Variables

When running from source, both the backend and frontend require separate environment configuration.

::: tip Generate Secure Secrets
You can generate secure random strings using:
- **Linux/macOS**: `openssl rand -hex 32`
- **Windows (PowerShell)**: `$bytes = New-Object Byte[] 32; [Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($bytes); -join ($bytes | ForEach-Object { "{0:x2}" -f $_ })`
:::

#### Backend Configuration

Copy the example file and configure:

```bash
cd rust-srec
cp .env.example .env
```

**Required Variables:**

| Variable | Description |
|----------|-------------|
| `JWT_SECRET` | Secret key for JWT token signing (min 32 characters) |

**Key Optional Variables:**

| Variable | Description | Default |
|----------|-------------|---------|
| `DATABASE_URL` | SQL database connection string | `sqlite:./srec.db` |
| `API_PORT` | Backend API server port | `8080` |
| `API_BIND_ADDRESS` | API bind address | `0.0.0.0` |
| `OUTPUT_DIR` | Directory for recordings | `./output` |
| `RUST_LOG` | Logging level | `info` |

See [`.env.example`](https://github.com/hua0512/rust-srec/blob/main/rust-srec/.env.example) for all available options.

---

#### Frontend Configuration

Copy the example file and configure:

```bash
cd rust-srec/frontend
cp .env.example .env
```

**Required Variables:**

| Variable | Description |
|----------|-------------|
| `SESSION_SECRET` | Secret for session encryption (min 32 characters) |

**Key Optional Variables:**

| Variable | Description | Default |
|----------|-------------|---------|
| `VITE_API_BASE_URL` | Backend API URL (build-time) | `http://localhost:8080/api` |
| `BACKEND_URL` | Backend URL for SSR (runtime) | `http://localhost:8080` |
| `COOKIE_SECURE` | Force HTTPS-only cookies | `auto` |

See [`.env.example`](https://github.com/hua0512/rust-srec/blob/main/rust-srec/frontend/.env.example) for all available options.

---

#### Proxy Configuration

If you're behind a corporate proxy or in a region with restricted access, add these to the **backend** `.env`:

| Variable | Description | Example |
|----------|-------------|---------|
| `HTTP_PROXY` | HTTP proxy server URL | `http://proxy.example.com:8080` |
| `HTTPS_PROXY` | HTTPS proxy server URL | `http://proxy.example.com:8080` |
| `NO_PROXY` | Comma-separated hosts to bypass proxy | `localhost,127.0.0.1` |

After starting the application, enable proxy in **Global Settings** > **Downloader** > **Proxy**.

::: tip Full Reference
For a complete list of all available environment variables, see the [Configuration Reference](./configuration.md#environment-variables).
:::

### Running the Application

Once built and configured:

**1. Start the Backend:**
```bash
cd rust-srec
./target/release/rust-srec
```

**2. Start the Frontend (Development):**
```bash
cd rust-srec/frontend
npm install
npm run dev
```

**3. Access the Application:**
- Frontend: `http://localhost:3000`
- API: `http://localhost:8080/api`

