# Platforms CLI

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
cargo build --release -p platforms-cli
```

## Usage

### Basic Extraction

```bash
# Extract media info from a URL (includes extras by default)
platforms-cli extract --url "https://twitch.tv/example_channel"

# Auto-select best quality stream
platforms-cli extract --url "https://live.bilibili.com/123456" --auto-select

# Filter streams by quality
platforms-cli extract --url "https://douyu.com/123456" --quality "1080p"

# Output in JSON format
platforms-cli extract --url "https://huya.com/123456" --output json

# Save to file
platforms-cli extract --url "https://twitch.tv/example_channel" --output-file result.json

# Exclude extra metadata
platforms-cli extract --url "https://twitch.tv/example_channel" --no-extras
```

### Batch Processing

```bash
# Process multiple URLs from a file
platforms-cli batch --input urls.txt --output-dir ./results

# Limit concurrent extractions
platforms-cli batch --input urls.txt --max-concurrent 3

# Continue on errors
platforms-cli batch --input urls.txt --continue-on-error
```

### Platform Information

```bash
# List supported platforms
platforms-cli platforms

# Show detailed platform information
platforms-cli platforms --detailed
```

### Configuration

```bash
# Show current configuration
platforms-cli config --show

# Reset to defaults
platforms-cli config --reset

# Use custom config file
platforms-cli --config ~/.config/platforms-cli/config.toml extract --url "https://twitch.tv/channel_name"
```

### Shell Completions

```bash
# Generate bash completions
platforms-cli completions bash

# Generate zsh completions
platforms-cli completions zsh

# Generate fish completions
platforms-cli completions fish

# Generate PowerShell completions
platforms-cli completions powershell
```

## Global Options

- `--verbose` / `-v` - Enable verbose output
- `--quiet` / `-q` - Suppress all output except errors
- `--config` / `-c` - Custom configuration file path
- `--timeout` - Request timeout in seconds (default: 30)
- `--retries` - Number of retry attempts (default: 3)

## Configuration File

The CLI tool supports configuration files in TOML format. Default location:

- **Windows**: `%APPDATA%\platforms-cli\config.toml`
- **macOS**: `~/Library/Application Support/platforms-cli/config.toml`
- **Linux**: `~/.config/platforms-cli/config.toml`

Example configuration:

```toml
default_output_format = "json"
default_timeout = 45
default_retries = 5
max_concurrent = 10
auto_select = true
include_extras = true  # Extras are included by default
colored_output = true
user_agent = "platforms-cli/1.0.0"

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
platforms-cli extract --url "https://twitch.tv/channel" --no-extras
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
cargo build --release -p platforms-cli
```

### Testing

```bash
cargo test -p platforms-cli
```

## License

This project is licensed under the MIT OR Apache-2.0 license.
