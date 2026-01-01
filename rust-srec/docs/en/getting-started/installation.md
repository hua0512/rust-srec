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

### Prerequisites

- **Rust**: 2024 edition (stable)
- **Git**
- **CMake**: Required for building aws-lc-rs
- **C/C++ Compiler**: 
  - Linux: GCC or Clang
  - Windows: MSVC (Visual Studio Build Tools)
  - macOS: Xcode Command Line Tools

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
