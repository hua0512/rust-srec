# Development Environment

Prerequisites for building and testing rust-srec from a clone on Linux, Windows or macOS. CI is the source of truth for exact versions and packages; this page follows [`pr.yml`](https://github.com/hua0512/rust-srec/blob/main/.github/workflows/pr.yml) and the release workflows. For running a build as a user, see [Build from Source](../getting-started/installation.md#build-from-source).

## What You Need

| Component | Needs |
|---|---|
| Backend and CLIs (`rust-srec`, `strev`, `mesio`, `crates/*`) | Git, [rustup](https://rustup.rs/), a C/C++ compiler |
| Web UI (`rust-srec/frontend`) and docs site (`rust-srec/docs`) | Node.js and pnpm |
| Desktop app (`rust-srec/src-tauri`) | All of the above, plus the Tauri system libraries for your OS |
| Tool-gated tests and local recording features | FFmpeg/FFprobe, optionally Streamlink and rclone ([below](#external-tools)) |

**Rust.** The toolchain is pinned in [`rust-toolchain.toml`](https://github.com/hua0512/rust-srec/blob/main/rust-toolchain.toml), with `clippy` and `rustfmt`. After installing rustup, run `rustup toolchain install` in the repository root to install it; rustup also selects it automatically for every `cargo` command in the repository. The minimum supported Rust version is the `rust-version` in the workspace [`Cargo.toml`](https://github.com/hua0512/rust-srec/blob/main/Cargo.toml).

**Node.js and pnpm.** Use the Node.js major version in [`.nvmrc`](https://github.com/hua0512/rust-srec/blob/main/.nvmrc) and the pnpm version in the `packageManager` field of `rust-srec/frontend/package.json` and `rust-srec/docs/package.json`. Node.js 25 and later no longer bundle Corepack, so install one of them yourself:

```bash
npm install -g corepack && corepack enable   # pnpm then follows packageManager
# or
npm install -g pnpm@12.4.1                   # the current packageManager version
```

**No `protoc`.** The Rust code generated from the `.proto` schemas is committed, so no Protobuf compiler is needed. To regenerate it after a schema change, follow [CONTRIBUTING.md](https://github.com/hua0512/rust-srec/blob/main/CONTRIBUTING.md#common-commands-repo-root).

## Linux

The commands below are for Debian and Ubuntu, which CI uses. Install the compiler and the OpenSSL headers:

```bash
sudo apt-get update
sudo apt-get install -y build-essential pkg-config libssl-dev
```

The default build does not use OpenSSL. `pkg-config` and `libssl-dev` are needed for the `tls-native-fallback` feature, which Linux CI, the release binaries and the Docker image enable:

```bash
cargo build --locked -p rust-srec --features tls-native-fallback
```

For the desktop app, install the Tauri system libraries from the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/#linux) for your distribution. On Debian and Ubuntu:

```bash
sudo apt-get install -y libwebkit2gtk-4.1-dev build-essential curl wget file \
  libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev
```

The release workflow builds the Linux desktop packages on Ubuntu 22.04 with `libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf`. The same page lists the Arch and Fedora packages.

## Windows

1. Install the [Microsoft C++ Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) and select the **Desktop development with C++** workload. Rust's MSVC toolchain links with it.
2. Install Rust with [`rustup-init.exe`](https://rustup.rs/) and keep the default MSVC host (`x86_64-pc-windows-msvc` on x64).
3. For the desktop app, WebView2 is already part of Windows 10 (version 1803 and later) and Windows 11. On older systems, install the Evergreen Bootstrapper from the [WebView2 download page](https://developer.microsoft.com/en-us/microsoft-edge/webview2/#download-section).

Use the default features on Windows, as CI does. `--all-features` enables `static-ssl`, which compiles a vendored OpenSSL from source, and a standard Windows setup lacks its build prerequisites.

## macOS

Install the Xcode Command Line Tools:

```bash
xcode-select --install
```

They are sufficient for the backend, the CLIs and the desktop app. CI tests on both Apple Silicon and Intel runners with no further packages.

## External Tools {#external-tools}

The default test suite needs no external programs. Some backend tests are marked `#[ignore]` because they run real tools; on Ubuntu, CI installs them with:

```bash
sudo apt-get install -y ffmpeg rclone
python -m pip install --only-binary=:all: -r rust-srec/tests/e2e/streamlink-requirements.txt
```

The FFmpeg tests need `ffmpeg` and `ffprobe` on `PATH`, and some need FFmpeg built with `libx264` and `libass`. The CI command for these tests is the "gated tests" step in `pr.yml`.

The same tools are optional at runtime: install them when you run the backend locally with FFmpeg processors, the Streamlink engine or rclone uploads. See [External Tools](../getting-started/installation.md#external-tools) and the [path variables](../reference/environment.md#external-tools).

## Verify the Setup

From the repository root:

```bash
cargo build --locked
cargo nextest run --locked -p rust-srec   # or: cargo test --locked -p rust-srec
```

Web UI, which talks to a running backend; set up both `.env` files as described in [Run the Frontend for Development](../getting-started/installation.md#run-the-frontend-for-development):

```bash
pnpm -C rust-srec/frontend install --frozen-lockfile
pnpm -C rust-srec/frontend dev
```

Desktop app, with the Tauri CLI version CI uses (run `pnpm setup` first if pnpm has no global bin directory yet):

```bash
pnpm add --global @tauri-apps/cli@2.11.4
cd rust-srec
tauri dev
```

Alternatively, `cargo install --locked tauri-cli --version "^2"` provides the same commands as `cargo tauri dev`. Either starts the desktop frontend dev server itself. [CONTRIBUTING.md](https://github.com/hua0512/rust-srec/blob/main/CONTRIBUTING.md) has the full command reference: formatting, Clippy, workspace tests, doctests, frontend checks and desktop builds.

## Faster Local Builds (Optional) {#faster-local-builds}

### cargo-nextest

CI runs tests with [cargo-nextest](https://nexte.st/), which runs each test in its own process and in parallel:

```bash
cargo install --locked cargo-nextest
```

Nextest does not run doctests; use `cargo test --locked -p <package> --doc` for those.

### Linker

Since Rust 1.90, `x86_64-unknown-linux-gnu` links with the bundled LLD by default, so x86_64 Linux needs no change. [mold](https://github.com/rui314/mold) can be faster still. To use it, install `mold` and `clang` from your distribution and add the following to your own `~/.cargo/config.toml` (not to the repository):

```toml
[target.'cfg(target_os = "linux")']
linker = "clang"
rustflags = ["-C", "link-arg=-fuse-ld=mold"]
```

### Shared Build Cache Across Worktrees

The maintainers use [huacnlee/cargo](https://huacnlee.github.io/cargo/) locally. It is an unofficial, third-party fork of Cargo, not affiliated with the Rust project. It shares build output between all git worktrees of a repository, and registry dependencies between projects. In our measurement, a test build in a new worktree at the same commit took about 44 s, against about 10 minutes for a cold build with stock Cargo.

Install:

```bash
# Linux and macOS
curl -fsSL https://huacnlee.github.io/cargo/install.sh | sh
```

```powershell
# Windows (PowerShell)
irm https://huacnlee.github.io/cargo/install.ps1 | iex
```

Before installing, note:

- It replaces `~/.cargo/bin/cargo` and keeps the rustup proxy as `cargo.rustup`, so `rust-toolchain.toml` and `cargo +toolchain` keep working. If a nightly toolchain is installed, the script also adds the Cranelift component to it.
- It updates itself in the background at most once a day. Set `CARGO_FORK_AUTO_UPDATE=0` to turn this off.
- It ignores `CARGO_TARGET_DIR`, `--target-dir` and `build.target-dir`, with a warning. Intermediate build output goes to `~/.cargo/targets`, while final binaries still appear in each worktree's `./target`. This cache grows to tens of gigabytes for this workspace, so put `~/.cargo/targets` on a drive with enough space.
- Upstream has seen less use on Windows than on Linux and macOS.

To uninstall and restore the rustup proxy, run the install script with `--uninstall` (`-Uninstall` on Windows):

```bash
curl -fsSL https://huacnlee.github.io/cargo/install.sh | sh -s -- --uninstall
```

```powershell
& ([scriptblock]::Create((irm https://huacnlee.github.io/cargo/install.ps1))) -Uninstall
```
