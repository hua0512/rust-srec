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

### 前置要求

- **Rust**：2024 edition（stable）
- **Git**
- **CMake**：编译 aws-lc-rs 所需
- **C/C++ 编译器**：
  - Linux：GCC 或 Clang
  - Windows：MSVC（Visual Studio 构建工具）
  - macOS：Xcode 命令行工具

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
