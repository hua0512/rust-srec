# 开发环境

在 Linux、Windows 或 macOS 上从源码克隆构建和测试 rust-srec 所需的环境。具体版本和软件包以 CI 为准；本页依据 [`pr.yml`](https://github.com/hua0512/rust-srec/blob/main/.github/workflows/pr.yml) 和发布工作流编写。以用户身份运行自行构建的版本，请参阅[从源码构建](../getting-started/installation.md#从源码构建)。

## 所需组件 {#所需组件}

| 组件 | 需要 |
|---|---|
| 后端和命令行工具（`rust-srec`、`strev`、`mesio`、`crates/*`） | Git、[rustup](https://rustup.rs/)、C/C++ 编译器 |
| Web UI（`rust-srec/frontend`）和文档站（`rust-srec/docs`） | Node.js 和 pnpm |
| 桌面应用（`rust-srec/src-tauri`） | 以上全部，以及所用操作系统的 Tauri 系统库 |
| 依赖外部工具的测试和本地录制功能 | FFmpeg/FFprobe，可选 Streamlink 和 rclone（见[下文](#外部工具)） |

**Rust。** 工具链固定在 [`rust-toolchain.toml`](https://github.com/hua0512/rust-srec/blob/main/rust-toolchain.toml) 中，并包含 `clippy` 和 `rustfmt`。安装 rustup 后，在仓库根目录运行 `rustup toolchain install` 安装该工具链；在仓库内执行的每条 `cargo` 命令也会由 rustup 自动选用它。最低支持的 Rust 版本是工作区 [`Cargo.toml`](https://github.com/hua0512/rust-srec/blob/main/Cargo.toml) 中的 `rust-version`。

**Node.js 和 pnpm。** 使用 [`.nvmrc`](https://github.com/hua0512/rust-srec/blob/main/.nvmrc) 中的 Node.js 主版本，以及 `rust-srec/frontend/package.json` 和 `rust-srec/docs/package.json` 的 `packageManager` 字段中的 pnpm 版本。Node.js 25 及以后的版本不再自带 Corepack，请自行安装以下任一工具：

```bash
npm install -g corepack && corepack enable   # 之后 pnpm 版本跟随 packageManager
# 或
npm install -g pnpm@12.4.1                   # 当前 packageManager 中的版本
```

**无需 `protoc`。** 由 `.proto` 模式生成的 Rust 代码已提交到仓库，因此不需要 Protobuf 编译器。修改模式后如需重新生成，请按照 [CONTRIBUTING.md](https://github.com/hua0512/rust-srec/blob/main/CONTRIBUTING.md#common-commands-repo-root) 操作。

## Linux {#linux}

以下命令适用于 CI 所用的 Debian 和 Ubuntu。安装编译器和 OpenSSL 头文件：

```bash
sudo apt-get update
sudo apt-get install -y build-essential pkg-config libssl-dev
```

默认构建不使用 OpenSSL。`pkg-config` 和 `libssl-dev` 用于 `tls-native-fallback` 特性；Linux CI、发布的二进制文件和 Docker 镜像都启用了该特性：

```bash
cargo build --locked -p rust-srec --features tls-native-fallback
```

构建桌面应用时，请按照 [Tauri 前置要求](https://v2.tauri.app/start/prerequisites/#linux)为所用发行版安装 Tauri 系统库。Debian 和 Ubuntu：

```bash
sudo apt-get install -y libwebkit2gtk-4.1-dev build-essential curl wget file \
  libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev
```

发布工作流在 Ubuntu 22.04 上构建 Linux 桌面安装包，安装的是 `libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf`。Arch 和 Fedora 的软件包见同一页面。

## Windows {#windows}

1. 安装 [Microsoft C++ 生成工具](https://visualstudio.microsoft.com/visual-cpp-build-tools/)，并选择 **使用 C++ 的桌面开发** 工作负载。Rust 的 MSVC 工具链依靠它进行链接。
2. 使用 [`rustup-init.exe`](https://rustup.rs/) 安装 Rust，保留默认的 MSVC 主机（x64 上为 `x86_64-pc-windows-msvc`）。
3. 桌面应用所需的 WebView2 已内置于 Windows 10（1803 及以后版本）和 Windows 11。更旧的系统请从 [WebView2 下载页面](https://developer.microsoft.com/en-us/microsoft-edge/webview2/#download-section)安装 Evergreen Bootstrapper。

在 Windows 上与 CI 一样使用默认特性。`--all-features` 会启用 `static-ssl`，它从源码编译内置的 OpenSSL，而标准 Windows 环境缺少其构建前置条件。

## macOS {#macos}

安装 Xcode 命令行工具：

```bash
xcode-select --install
```

它足以构建后端、命令行工具和桌面应用。CI 在 Apple Silicon 和 Intel 运行器上测试时都不安装其他软件包。

## 外部工具 {#外部工具}

默认测试套件不需要外部程序。部分后端测试会运行真实工具，因此标记为 `#[ignore]`；CI 在 Ubuntu 上这样安装这些工具：

```bash
sudo apt-get install -y ffmpeg rclone
python -m pip install --only-binary=:all: -r rust-srec/tests/e2e/streamlink-requirements.txt
```

FFmpeg 相关测试需要 `PATH` 中有 `ffmpeg` 和 `ffprobe`，部分测试还要求 FFmpeg 包含 `libx264` 和 `libass`。运行这些测试的 CI 命令见 `pr.yml` 中的 “gated tests” 步骤。

运行时这些工具同样是可选的：在本地运行后端并使用 FFmpeg 处理器、Streamlink 引擎或 rclone 上传时再安装。参阅[外部工具](../getting-started/installation.md#外部工具)和[路径变量](../reference/environment.md#外部工具)。

## 验证环境 {#验证环境}

在仓库根目录：

```bash
cargo build --locked
cargo nextest run --locked -p rust-srec   # 或：cargo test --locked -p rust-srec
```

Web UI 需要连接正在运行的后端；请按照[以开发模式运行前端](../getting-started/installation.md#以开发模式运行前端)配置两个 `.env` 文件：

```bash
pnpm -C rust-srec/frontend install --frozen-lockfile
pnpm -C rust-srec/frontend dev
```

桌面应用使用与 CI 相同版本的 Tauri CLI（如果 pnpm 尚未配置全局 bin 目录，先运行 `pnpm setup`）：

```bash
pnpm add --global @tauri-apps/cli@2.11.4
cd rust-srec
tauri dev
```

也可以用 `cargo install --locked tauri-cli --version "^2"` 安装，命令相同，改为 `cargo tauri dev`。两种方式都会自动启动桌面前端开发服务器。完整命令参考见 [CONTRIBUTING.md](https://github.com/hua0512/rust-srec/blob/main/CONTRIBUTING.md)：格式化、Clippy、工作区测试、文档测试、前端检查和桌面构建。

## 更快的本地构建（可选） {#更快的本地构建}

### cargo-nextest {#cargo-nextest}

CI 使用 [cargo-nextest](https://nexte.st/) 运行测试，它让每个测试在独立进程中并行运行：

```bash
cargo install --locked cargo-nextest
```

Nextest 不运行文档测试；文档测试请使用 `cargo test --locked -p <package> --doc`。

### 链接器 {#链接器}

自 Rust 1.90 起，`x86_64-unknown-linux-gnu` 默认使用工具链自带的 LLD 链接，因此 x86_64 Linux 无需改动。[mold](https://github.com/rui314/mold) 可能更快。如需使用，从发行版安装 `mold` 和 `clang`，并在你自己的 `~/.cargo/config.toml`（而不是仓库）中加入：

```toml
[target.'cfg(target_os = "linux")']
linker = "clang"
rustflags = ["-C", "link-arg=-fuse-ld=mold"]
```

### 在工作树之间共享构建缓存 {#共享构建缓存}

维护者在本地使用 [huacnlee/cargo](https://huacnlee.github.io/cargo/)。它是非官方的第三方 Cargo 分支，与 Rust 项目无关。它在同一仓库的所有 git 工作树之间共享构建产物，并在项目之间共享注册表依赖。在我们的测量中，在同一提交的新工作树中构建测试约需 44 秒，而使用原版 Cargo 冷构建约需 10 分钟。

安装：

```bash
# Linux 和 macOS
curl -fsSL https://huacnlee.github.io/cargo/install.sh | sh
```

```powershell
# Windows（PowerShell）
irm https://huacnlee.github.io/cargo/install.ps1 | iex
```

安装前请注意：

- 它会替换 `~/.cargo/bin/cargo`，并把 rustup 代理保留为 `cargo.rustup`，因此 `rust-toolchain.toml` 和 `cargo +toolchain` 仍然有效。如果已安装 nightly 工具链，安装脚本还会为其添加 Cranelift 组件。
- 只有当 `~/.cargo/bin` 在 `PATH` 中排在最前时才会生效。发行版打包的 rustup（例如 Arch 的 `rustup` 软件包）会把 `cargo` 放在 `/usr/bin`，并且可能不把 `~/.cargo/bin` 加入 `PATH`。可以用 `cargo --version` 检查：启用该分支时输出中会出现 `huacnlee/cargo`。
- 它每天最多在后台自动更新一次。设置 `CARGO_FORK_AUTO_UPDATE=0` 可关闭自动更新。
- 它会忽略 `CARGO_TARGET_DIR`、`--target-dir` 和 `build.target-dir`，并给出警告。中间构建产物存放在 `~/.cargo/targets`，最终二进制文件仍出现在各工作树的 `./target` 中。对本工作区而言该缓存会增长到数十 GB，请把 `~/.cargo/targets` 放在空间充足的磁盘上。
- 上游在 Windows 上的使用少于 Linux 和 macOS。

卸载并恢复 rustup 代理时，使用 `--uninstall`（Windows 上为 `-Uninstall`）运行安装脚本：

```bash
curl -fsSL https://huacnlee.github.io/cargo/install.sh | sh -s -- --uninstall
```

```powershell
& ([scriptblock]::Create((irm https://huacnlee.github.io/cargo/install.ps1))) -Uninstall
```
