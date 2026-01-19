# 安装

## Docker（推荐）

Docker 是运行 Rust-Srec 最简单的推荐方式。

请参阅 [Docker 部署指南](./docker.md) 获取完整设置说明。

**快速开始：**
```bash
docker pull ghcr.io/hua0512/rust-srec:latest
```

## 预编译二进制

从 [GitHub Releases](https://github.com/hua0512/rust-srec/releases) 下载预编译二进制文件。

支持平台：
- Linux (x86_64, aarch64)
- Windows (x86_64)
- macOS (x86_64, aarch64)

## 从源码编译

### 环境要求

在从源码编译之前，请确保您的系统满足以下要求：

#### Rust 工具链

- **最低版本**：Rust 1.83.0（2024 edition）
- **频道**：Stable
- **安装**：使用 [rustup](https://rustup.rs/) 轻松安装和管理

```bash
# 使用 rustup 安装 Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# 或在 Windows 上，从 https://rustup.rs/ 下载并运行 rustup-init.exe

# 验证安装
rustc --version
cargo --version
```

#### 系统要求

- **内存**：至少 2GB 可用（推荐 4GB+ 以加快编译速度）
- **磁盘空间**：至少 2GB 空闲空间用于依赖和构建产物
- **网络**：需要互联网连接以下载依赖

### 编译前置要求

#### 必需工具

- **Git**：版本控制系统
  - Linux：`sudo apt-get install git`（Debian/Ubuntu）或 `sudo dnf install git`（Fedora/RHEL）
  - Windows：[下载 Git](https://git-scm.com/download/win)
  - macOS：包含在 Xcode 命令行工具中

- **CMake**：编译 aws-lc-rs 所需（最低版本 3.12）
  - 需要 3.12 或更高版本
  
- **C/C++ 编译器**：
  - **Linux**：GCC 7.1+ 或 Clang 5.0+
  - **Windows**：MSVC（Visual Studio 2017 或更高版本 / Visual Studio 构建工具）
  - **macOS**：Xcode 命令行工具（macOS 10.13+ SDK）

#### aws-lc-rs 依赖

Rust-Srec 使用 [aws-lc-rs](https://github.com/aws/aws-lc-rs) 进行加密。

完整的平台特定要求请参阅 [aws-lc-rs 官方文档](https://aws.github.io/aws-lc-rs/requirements/index.html)。

**Linux (Debian/Ubuntu)：**
```bash
sudo apt-get install cmake build-essential
```

**Linux (Fedora/RHEL)：**
```bash
sudo dnf install cmake gcc g++
```

**macOS：**
```bash
xcode-select --install
brew install cmake
```

**Windows：**
- 安装 [Visual Studio 构建工具](https://visualstudio.microsoft.com/visual-cpp-build-tools/)
- 安装 [CMake](https://cmake.org/download/)
- 确保两者都在 PATH 环境变量中

### 编译

```bash
# 克隆仓库
git clone https://github.com/hua0512/rust-srec.git
cd rust-srec

# 编译 release 版本
cargo build --release -p rust-srec

# 二进制文件位于 target/release/rust-srec
```

### 环境变量

从源码运行时，后端和前端需要分别配置环境变量。

::: tip 生成安全密钥
您可以使用以下方式生成安全随机字符串：
- **Linux/macOS**：`openssl rand -hex 32`
- **Windows (PowerShell)**：`$bytes = New-Object Byte[] 32; [Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($bytes); -join ($bytes | ForEach-Object { "{0:x2}" -f $_ })`
:::

#### 后端配置

复制示例文件并配置：

```bash
cd rust-srec
cp .env.example .env
```

**必需变量：**

| 变量 | 描述 |
|------|------|
| `JWT_SECRET` | JWT 令牌签名的密钥（至少 32 个字符） |

**主要可选变量：**

| 变量 | 描述 | 默认值 |
|------|------|--------|
| `DATABASE_URL` | SQL 数据库连接字符串 | `sqlite:./srec.db` |
| `API_PORT` | 后端 API 服务器端口 | `8080` |
| `API_BIND_ADDRESS` | API 绑定地址 | `0.0.0.0` |
| `OUTPUT_DIR` | 录播文件目录 | `./output` |
| `RUST_LOG` | 日志级别 | `info` |
| `WEB_PUSH_VAPID_PUBLIC_KEY` | 用于浏览器推送通知的 VAPID 公钥 | - |
| `WEB_PUSH_VAPID_PRIVATE_KEY` | 用于浏览器推送通知的 VAPID 私钥 | - |

查看 [`.env.example`](https://github.com/hua0512/rust-srec/blob/main/rust-srec/.env.example) 获取所有可用选项。

---

#### 前端配置

复制示例文件并配置：

```bash
cd rust-srec/frontend
cp .env.example .env
```

**必需变量：**

| 变量 | 描述 |
|------|------|
| `SESSION_SECRET` | 会话加密密钥（至少 32 个字符） |

**主要可选变量：**

| 变量 | 描述 | 默认值 |
|------|------|--------|
| `VITE_API_BASE_URL` | 后端 API URL（构建时） | `http://localhost:8080/api` |
| `BACKEND_URL` | SSR 后端 URL（运行时） | `http://localhost:8080` |
| `COOKIE_SECURE` | 强制 HTTPS-only cookies | `auto` |

查看 [`.env.example`](https://github.com/hua0512/rust-srec/blob/main/rust-srec/frontend/.env.example) 获取所有可用选项。

---

#### 代理配置

如果您在企业代理后或网络访问受限的地区，将以下变量添加到**后端** `.env`：

| 变量 | 描述 | 示例 |
|------|------|------|
| `HTTP_PROXY` | HTTP 代理服务器 URL | `http://proxy.example.com:8080` |
| `HTTPS_PROXY` | HTTPS 代理服务器 URL | `http://proxy.example.com:8080` |
| `NO_PROXY` | 绕过代理的主机列表（逗号分隔） | `localhost,127.0.0.1` |

启动应用程序后，在**全局设置** > **下载器** > **代理**中启用代理。

::: tip 完整参考
有关所有可用环境变量的完整列表，请参阅[配置参考](./configuration.md#环境变量)。
:::

### 运行应用程序

编译和配置完成后：

**1. 启动后端：**
```bash
cd rust-srec
./target/release/rust-srec
```

**2. 启动前端（开发模式）：**
```bash
cd rust-srec/frontend
pnpm install
pnpm dev
```

**3. 访问应用程序：**
- 前端：`http://localhost:3000`
- API：`http://localhost:8080/api`
