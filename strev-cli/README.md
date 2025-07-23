# Strev CLI

A high-performance, user-friendly CLI tool for extracting streaming media information from various online platforms.

## Features

### 🚀 Performance Optimizations

- **Async/await concurrency** - Non-blocking operations for better performance
- **HTTP client reuse** - Efficient connection pooling
- **Retry logic with exponential backoff** - Robust error handling
- **Configurable timeouts** - Avoid hanging requests
- **Batch processing** - Process multiple URLs concurrently

### 🎯 User Experience

- **Multiple output formats** - Pretty, JSON, CSV, Table
- **Interactive stream selection** - Choose from available streams
- **Stream filtering** - Filter by quality and format
- **Auto-selection mode** - Automatically pick best quality
- **Progress indicators** - Visual feedback for long operations
- **Colored output** - Enhanced readability
- **Configuration file support** - Persistent settings
- **Extras displayed by default** - Rich metadata included automatically

### 🛠 Advanced Features

- **Batch processing** - Handle multiple URLs from files
- **Shell completions** - For bash, zsh, fish, powershell
- **Verbose/quiet modes** - Configurable logging levels
- **File output** - Save results to files
- **Error recovery** - Continue processing on errors
- **Configuration management** - Show/reset configuration

## Installation

```bash
cargo build --release -p strev
```

## Usage

### Basic Extraction

```bash
# Extract media info from a URL (includes extras by default)
strev extract --url "https://twitch.tv/example_channel"

# Auto-select best quality stream
strev extract --url "https://live.bilibili.com/123456" --auto-select

# Filter streams by quality
strev extract --url "https://douyu.com/123456" --quality "1080p"

# Output in JSON format
strev extract --url "https://huya.com/123456" --output json

# Save to file
strev extract --url "https://twitch.tv/example_channel" --output-file result.json

# Exclude extra metadata
strev extract --url "https://twitch.tv/example_channel" --no-extras
```

### Batch Processing

```bash
# Process multiple URLs from a file
strev batch --input urls.txt --output-dir ./results

# Limit concurrent extractions
strev batch --input urls.txt --max-concurrent 3

# Continue on errors
strev batch --input urls.txt --continue-on-error
```

### Platform Information

```bash
# List supported platforms
strev platforms

# Show detailed platform information
strev platforms --detailed
```

### Configuration

```bash
# Show current configuration
strev config --show

# Reset to defaults
strev config --reset

# Use custom config file
strev --config ~/.config/strev/config.toml extract --url "https://twitch.tv/channel_name"
```

### Shell Completions

```bash
# Generate bash completions
strev completions bash

# Generate zsh completions
strev completions zsh

# Generate fish completions
strev completions fish

# Generate PowerShell completions
strev completions powershell
```

## Global Options

- `--verbose` / `-v` - Enable verbose output
- `--quiet` / `-q` - Suppress all output except errors
- `--config` / `-c` - Custom configuration file path
- `--timeout` - Request timeout in seconds (default: 30)
- `--retries` - Number of retry attempts (default: 3)
- `--proxy` - Proxy URL (supports http, https, socks5)
- `--proxy-username` - Proxy username (if proxy requires authentication)
- `--proxy-password` - Proxy password (if proxy requires authentication)

## Proxy Support

The CLI tool supports HTTP, HTTPS, and SOCKS5 proxies. You can configure proxies through command line arguments or configuration file.

### CLI Usage

```bash
# Use HTTP proxy
strev extract --url "https://twitch.tv/example_channel" --proxy "http://proxy.example.com:8080"

# Use HTTPS proxy
strev extract --url "https://bilibili.com/123456" --proxy "https://proxy.example.com:8080"

# Use SOCKS5 proxy
strev extract --url "https://douyu.com/123456" --proxy "socks5://proxy.example.com:1080"

# Use proxy with authentication
strev extract --url "https://huya.com/123456" \
  --proxy "http://proxy.example.com:8080" \
  --proxy-username "user" \
  --proxy-password "pass"

# Batch processing with proxy
strev batch --input urls.txt \
  --proxy "http://proxy.example.com:8080" \
  --output-dir ./results
```

### Configuration File Proxy Settings

```toml
# Default proxy settings
default_proxy = "http://proxy.example.com:8080"
default_proxy_username = "username"
default_proxy_password = "password"
```

## Configuration File

The CLI tool supports configuration files in TOML format. Default location:

- **Windows**: `%APPDATA%\strev\config.toml`
- **macOS**: `~/Library/Application Support/strev/config.toml`
- **Linux**: `~/.config/strev/config.toml`

Example configuration:

```toml
default_output_format = "json"
default_timeout = 45
default_retries = 5
max_concurrent = 10
auto_select = true
include_extras = true  # Extras are included by default
colored_output = true
user_agent = "strev/1.0.0"

[default_cookies]
# Platform-specific default cookies
```

## Environment Variables

Configuration can also be set via environment variables with the `PLATFORMS_CLI_` prefix:

```bash
export PLATFORMS_CLI_DEFAULT_TIMEOUT=60
export PLATFORMS_CLI_AUTO_SELECT=true
export PLATFORMS_CLI_INCLUDE_EXTRAS=false  # Disable extras if needed
export PLATFORMS_CLI_COLORED_OUTPUT=false
```

## Supported Platforms

| Platform | URL Examples | Description |
|----------|-------------|-------------|
| **Bilibili** | `live.bilibili.com/{room_id}` | Live streaming platform |
| **Douyin** | `live.douyin.com/{room_id}` | TikTok China live streaming |
| **Douyu** | `douyu.com/{room_id}` | Gaming live streaming |
| **Huya** | `huya.com/{room_id}` | Gaming live streaming |
| **Twitch** | `twitch.tv/{channel_name}` | Gaming and creative content |
| **Weibo** | `weibo.com/u/{user_id}`, `weibo.com/l/wblive/p/show/{live_id}` | Social media live streaming |
| **Redbook** | `xiaohongshu.com/user/profile/{user_id}`, `xhslink.com/{share_id}` | Lifestyle live streaming |
| **TikTok** | `tiktok.com/@{username}/live` | Short-form video live streaming |
| **Twitcasting** | `twitcasting.tv/{username}` | Live broadcasting service |
| **Picarto** | `picarto.tv/{channel_name}` | Art streaming platform |
| **PandaTV** | `pandalive.co.kr/play/{user_id}` | Live streaming platform (Defunct) |

## Output Formats

### Pretty (Default)

Human-readable colored output with clear formatting. **Includes extras by default**.

### JSON

Structured JSON output for programmatic use. **Includes extras by default**.

### JSON Compact

Minified JSON output for reduced size. **Includes extras by default**.

### Table

Formatted table output for easy reading.

### CSV

Comma-separated values for spreadsheet import.

## Extras Information

**By default, the CLI tool displays extra metadata** such as:

- Platform-specific stream information
- Additional media properties
- Technical details and parameters
- Custom metadata from the platform

To exclude extras from the output, use the `--no-extras` flag:

```bash
strev extract --url "https://twitch.tv/channel" --no-extras
```

## Development

### Dependencies

- `tokio` - Async runtime
- `clap` - Command-line argument parsing
- `serde` - Serialization/deserialization
- `reqwest` - HTTP client
- `colored` - Terminal colors
- `indicatif` - Progress bars
- `inquire` - Interactive prompts
- `config` - Configuration management
- `tracing` - Structured logging

### Building

```bash
cargo build --release -p strev
```

### Testing

```bash
cargo test -p strev
```

## License

This project is licensed under the MIT OR Apache-2.0 license.
