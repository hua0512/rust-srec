# 安装

## 选择部署方式

| 方式 | 适用场景 | Web 界面 | API |
|---|---|---|---|
| [Docker](./docker.md) | 大多数用户和生产主机 | `http://localhost:15275` | `http://localhost:12555/api` |
| 预编译二进制 | 仅后端或自定义部署 | 需单独部署前端 | 后端默认：`http://localhost:12555/api` |
| [systemd 服务](#systemd-服务-linux) | 不使用 Docker 运行后端的 Linux 主机 | 需单独部署前端 | `http://localhost:12555/api` |
| 使用示例 `.env` 的源码工作区 | 开发与贡献 | `http://localhost:15275` | `http://localhost:8080/api` |

Docker 会把前端容器的 `80` 端口和后端容器的 `8080` 端口映射到表中的宿主机端口。容器内部端口不是宿主机浏览器应访问的地址。

## Docker（推荐）

Docker 已打包后端、前端和所需运行时依赖。请先按 [Docker 部署指南](./docker.md)安装，再继续[完成第一次录制](./first-recording.md)。

## 预编译二进制

从 [GitHub Releases](https://github.com/hua0512/rust-srec/releases) 下载适合操作系统和架构的发布包。

`rust-srec` 可执行文件运行后端。完整的浏览器使用体验还需要单独部署前端，或直接采用 Docker 部署。向其他机器开放后端之前，必须生成至少 32 字符且唯一的 `JWT_SECRET`。

## systemd 服务（Linux）

仓库提供了 `rust-srec/rust-srec.service`，这是一个已做安全加固的 unit，用于把预编译后端二进制作为系统服务运行。它只托管后端；完整的浏览器使用体验仍需单独部署前端，或改用 Docker 部署。

### 安装

以 root 身份，在存放已下载的 `rust-srec` 二进制和 `rust-srec.service` 的目录中执行：

```bash
useradd --system --home-dir /var/lib/rust-srec --shell /usr/sbin/nologin rust-srec
install -D -m 0755 rust-srec /opt/rust-srec/rust-srec
install -d -m 0750 -o root -g rust-srec /etc/rust-srec
umask 027 && printf 'JWT_SECRET=%s\n' "$(openssl rand -hex 32)" \
  > /etc/rust-srec/rust-srec.env
chown root:rust-srec /etc/rust-srec/rust-srec.env
install -D -m 0644 rust-srec.service /etc/systemd/system/rust-srec.service
systemctl daemon-reload && systemctl enable --now rust-srec
```

该账号的家目录必须是 `/var/lib/rust-srec`，且必须可写：systemd 会从 passwd 读取 `$HOME`，rclone 和 BaiduPCS-Go 都会在其中改写会话文件。`ProtectSystem=strict` 让 `/opt` 只读，因此安装目录不能作为家目录。

`StateDirectory=` 和 `LogsDirectory=` 会在每次启动时创建 `/var/lib/rust-srec`、`/var/lib/rust-srec/output` 和 `/var/log/rust-srec`，全新主机无需预先准备任何目录。

`/etc/rust-srec/rust-srec.env` 通过 `EnvironmentFile=` 加载。服务缺少 `JWT_SECRET` 会拒绝启动，因此该文件实际上是必需的。Web Push（VAPID）密钥，以及 ffmpeg、rclone、streamlink 或 DanmakuFactory 的 `*_PATH` 覆盖项也应写在这里。文件中的每一项都会覆盖 unit 自身的 `Environment=` 设置。

只有当 `/var/lib/rust-srec` 或 `/var/log/rust-srec` 是上一次安装遗留下来的目录时，才需要在首次启动前修正所有权：

```bash
[ -d /var/lib/rust-srec ] && chown -R rust-srec:rust-srec /var/lib/rust-srec
[ -d /var/log/rust-srec ] && chown -R rust-srec:rust-srec /var/log/rust-srec
```

`StateDirectory=` 和 `LogsDirectory=` 发现目录属主与自身不一致时会递归 chown，且该操作发生在 exec setup 阶段；在庞大的录制目录树上，这次遍历可能超过管理器默认的启动超时。

`/opt/rust-srec` 同时也是 Docker Compose 安装脚本的默认目录（`RUST_SREC_DIR`）。两种部署共用一台主机时，请为二进制选择其他路径。

验证服务：

```bash
systemctl status rust-srec
journalctl -u rust-srec -f
curl http://localhost:12555/api/health/live
```

### 设置录制目录

::: warning 未设置输出文件夹时录制必然失败
数据库出厂时 `output_folder` 为 `/app/output`，该路径属于 Docker 镜像，`ProtectSystem=strict` 既不会提供它也不会让它可写，且没有任何环境变量能覆盖它。在**全局设置 > 输出文件夹**填入 `/var/lib/rust-srec/output` 或 `ReadWritePaths=` 列出的卷之前，服务能正常启动，但每次录制都会失败。
:::

`RUST_SREC_OUTPUT_ROOTS` 必须与该值保持一致，否则输出根写入门控会把服务报告为降级。unit 出厂时两者都指向 `/var/lib/rust-srec/output`。

录制量超过几百 GB 时，应为其单独准备一个卷并写入 `ReadWritePaths=`，而不是嵌套在 `/var/lib/rust-srec` 内部。`ReadWritePaths=` 中的每个路径都必须已存在——路径缺失时 `ProtectSystem=strict` 会让 unit 以 `226/NAMESPACE` 失败——而且 systemd 不会为它们 chown，因此创建时就要归属 `rust-srec`。

### unit 设置的变量

| 变量 | 值 |
|---|---|
| `DATABASE_URL` | `sqlite:///var/lib/rust-srec/rust-srec.db` |
| `LOG_DIR` | `/var/log/rust-srec` |
| `OUTPUT_DIR` | `/var/lib/rust-srec/output` |
| `RUST_SREC_OUTPUT_ROOTS` | `/var/lib/rust-srec/output` |
| `API_BIND_ADDRESS` | `0.0.0.0` |
| `API_PORT` | `12555` |
| `RUST_LOG` | `info` |
| `RUST_SREC_SHUTDOWN_TIMEOUT_SECS` | `30` |

以上都可以在 `/etc/rust-srec/rust-srec.env` 中覆盖。`TimeoutStopSec=35` 必须始终大于 `RUST_SREC_SHUTDOWN_TIMEOUT_SECS`；只调整其中一个，会导致 systemd 在录制尚未收尾时就发送 `SIGKILL`。

### 文件权限

`UMask=0027` 让录制文件写为 `-rw-r-----`，只有 `rust-srec` 及其用户组可读。需要读取这些文件的账号（文件服务、媒体库扫描器等）应加入 `rust-srec` 组，或在 drop-in 中调低 `UMask=`。unit 的其余加固措施及其对 GPU 转码的影响见[安全](../operations/security.md)。

## 从源码构建

### 环境要求

- Stable 渠道的 Rust 1.95 或更高版本。这是工作区 `Cargo.toml` 中声明的 `rust-version`，版本低于它 Cargo 会直接拒绝构建。
- Git、CMake 3.12 或更高版本，以及 C/C++ 编译器。
- 运行前端时需要 Node.js 26，以及 `rust-srec/frontend/package.json` 声明的 pnpm 版本。
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
