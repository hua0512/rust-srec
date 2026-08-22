# 安装

## 选择部署方式

| 方式 | 适用场景 | Web 界面 | API |
|---|---|---|---|
| [Docker](./docker.md) | 大多数用户和生产主机 | `http://localhost:15275` | `http://localhost:12555/api` |
| 预编译二进制 | 仅后端或自定义部署 | 需单独部署前端 | 后端默认：`http://localhost:12555/api` |
| 使用示例 `.env` 的源码工作区 | 开发与贡献 | `http://localhost:15275` | `http://localhost:8080/api` |

Docker 会把前端容器的 `80` 端口和后端容器的 `8080` 端口映射到表中的宿主机端口。容器内部端口不是宿主机浏览器应访问的地址。

## Docker（推荐）

Docker 已打包后端、前端和所需运行时依赖。请先按 [Docker 部署指南](./docker.md)安装，再继续[完成第一次录制](./first-recording.md)。

## 预编译二进制

从 [GitHub Releases](https://github.com/hua0512/rust-srec/releases) 下载适合操作系统和架构的发布包。

`rust-srec` 可执行文件运行后端。完整的浏览器使用体验还需要单独部署前端，或直接采用 Docker 部署。向其他机器开放后端之前，必须生成至少 32 字符且唯一的 `JWT_SECRET`。

## 从源码构建

### 环境要求

- Stable 渠道的 Rust 1.95 或更高版本。这是工作区 `Cargo.toml` 中声明的 `rust-version`，版本低于它 Cargo 会直接拒绝构建。
- Git、CMake 3.12 或更高版本，以及 C/C++ 编译器。
- 运行前端时需要 Node.js 24，以及 `rust-srec/frontend/package.json` 声明的 pnpm 版本。
- 至少 2 GB 空闲磁盘空间用于依赖和构建产物。

按操作系统安装原生构建工具：

```bash
# Debian 或 Ubuntu
sudo apt-get install git cmake build-essential

# Fedora 或 RHEL
sudo dnf install git cmake gcc g++

# macOS
xcode-select --install
brew install cmake
```

Windows 请安装带 C++ 工作负载的 [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) 和 [CMake](https://cmake.org/download/)。原生编译失败时可核对 [aws-lc-rs 要求](https://aws.github.io/aws-lc-rs/requirements/index.html)。

### 构建并配置后端

```bash
git clone https://github.com/hua0512/rust-srec.git
cd rust-srec
cargo build --locked --release -p rust-srec

cd rust-srec
cp .env.example .env
```

把 `.env` 中的 `JWT_SECRET` 替换为至少 32 字符的随机值。仓库示例设置了 `API_PORT=8080`；不加载 `.env` 时，后端默认使用 `12555`。

主要后端设置：

| 变量 | 用途 | 示例文件值 |
|---|---|---|
| `JWT_SECRET` | 签发访问和刷新令牌；必填 | 必须替换占位值 |
| `DATABASE_URL` | SQLite 数据库位置 | `sqlite:./srec.db` |
| `API_BIND_ADDRESS` | 监听的网络接口 | `0.0.0.0` |
| `API_PORT` | API 端口 | `8080` |
| `OUTPUT_DIR` | 录制输出目录 | `./output` |
| `RUST_LOG` | 日志级别 | `info` |

可用 `openssl rand -hex 32` 生成密钥。PowerShell 命令如下：

```powershell
$bytes = New-Object Byte[] 32
[Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($bytes)
-join ($bytes | ForEach-Object { "{0:x2}" -f $_ })
```

从仓库根目录启动后端：

```bash
./target/release/rust-srec
```

### 以开发模式运行前端

```bash
cd rust-srec/frontend
cp .env.example .env
pnpm install --frozen-lockfile
pnpm dev
```

请替换前端 `.env` 中的 `SESSION_SECRET`。其中 API 已指向示例后端端口 `8080`；Vite 开发界面使用 `15275` 端口。

## 外部工具

内置 Mesio 下载引擎不依赖 Streamlink。只有选择相应功能时才需要安装外部工具：

- 非 Docker 环境使用 FFmpeg 处理器或基于 FFmpeg 的工作流时，安装 `ffmpeg` 并加入 `PATH`。
- 选择 Streamlink 下载引擎时，安装 `streamlink` 并加入 `PATH`。
- 使用 `baidupcs` 百度网盘上传处理器时，安装 [BaiduPCS-Go](https://github.com/qjfoidnh/BaiduPCS-Go) 并加入 `PATH`（或通过 `BAIDUPCS_PATH` 指定路径）。
- Docker 镜像已经包含其支持的运行时工具，标准 Docker 部署不需要在宿主机重复安装。

## 下一步

打开所选部署方式对应的 Web 地址，并按[完成第一次录制](./first-recording.md)操作。面向公网或长期运行前，请先完成[生产部署](../operations/production.md)检查清单。
