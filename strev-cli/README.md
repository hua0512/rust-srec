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
- **Configuration management** - Show/reset configuration
- **Rich output for resolved streams** - `resolve` command supports all output formats

## Installation

```bash
cargo build --release -p strev
```

## Commands

This section provides a detailed overview of all available commands and their options.

### Global Options

These options can be used with any command.

| Option                  | Short | Description                               | Default |
| ----------------------- | ----- | ----------------------------------------- | ------- |
| `--verbose`             | `-v`  | Enable verbose (debug) output.            | `false` |
| `--quiet`               | `-q`  | Suppress all output except errors.        | `false` |
| `--config <PATH>`       | `-c`  | Path to a custom configuration file.      | (none)  |
| `--timeout <SECONDS>`   |       | Request timeout in seconds.               | `30`    |
| `--retries <NUM>`       |       | Number of retry attempts on failure.      | `3`     |
| `--proxy <URL>`         |       | Proxy URL (supports http, https, socks5). | (none)  |
| `--proxy-username <USER>`|      | Username for proxy authentication.        | (none)  |
| `--proxy-password <PASS>`|      | Password for proxy authentication.        | (none)  |

---

### `extract`

Extracts media information from a single URL.

**Usage:** `strev extract [OPTIONS]`

**Options:**

| Option                 | Short | Description                                        | Default  |
| ---------------------- | ----- | -------------------------------------------------- | -------- |
| `--url <URL>`          | `-u`  | **Required.** The URL of the media to extract.     |          |
| `--cookies <COOKIES>`  |       | Cookies to use for the request.                    | (none)   |
| `--extras <JSON>`      |       | Extra parameters for the extractor (JSON string).  | (none)   |
| `--output <FORMAT>`    | `-o`  | Output format (`pretty`, `json`, `json-compact`, `table`, `csv`). | `pretty` |
| `--output-file <PATH>` | `-O`  | Save the output to a file instead of `stdout`.     | (none)   |
| `--quality <QUALITY>`  |       | Filter streams by quality (e.g., "1080p").         | (none)   |
| `--format <FORMAT>`    |       | Filter streams by format (e.g., "mp4", "flv").     | (none)   |
| `--auto-select`        |       | Auto-select the best quality stream without a prompt.| `false`  |

#### Behavior

*   **Interactive Mode:** By default (when `output` is `pretty` or `table` and `--auto-select` is not used), if multiple media streams are found, it will prompt the user to select one. This interactive prompt is disabled if the output is not to a TTY.
*   **Auto-Selection:** When `--auto-select` is specified, it automatically selects the stream with the highest priority value.

---

### `batch`

Processes multiple URLs from a file in parallel.

**Usage:** `strev batch [OPTIONS]`

**Options:**

| Option                    | Short | Description                                    | Default |
| ------------------------- | ----- | ---------------------------------------------- | ------- |
| `--input <PATH>`          | `-i`  | **Required.** Input file with one URL per line.|         |
| `--output-dir <PATH>`     | `-o`  | Directory to save output files.                | (none)  |
| `--output-format <FORMAT>`| `-f`  | Output format for the results.                 | `json`  |
| `--max-concurrent <NUM>`  |       | Maximum number of concurrent extractions.      | `5`     |
| `--continue-on-error`     |       | Continue processing even if some URLs fail.    | `false` |

#### Behavior

*   Reads URLs from the specified input file, ignoring empty lines and lines starting with `#`.
*   Always uses **auto-selection** for streams, choosing the one with the highest bitrate and priority.
*   If `--output-dir` is specified, results are saved to `batch_results.json` (for JSON formats) or `batch_summary.txt` (for other formats) in that directory.

---

### `resolve`

Resolves a final stream URL from a stream data payload, which is typically obtained from a previous `extract` command.

**Usage:** `strev resolve [OPTIONS]`

**Options:**

| Option                 | Short | Description                                        | Default  |
| ---------------------- | ----- | -------------------------------------------------- | -------- |
| `--url <URL>`          | `-u`  | **Required.** The original URL of the media.       |          |
| `--payload <JSON>`     |       | Stream information payload (JSON string). Reads from `stdin` if not provided. | (none)   |
| `--cookies <COOKIES>`  |       | Cookies to use for the request.                    | (none)   |
| `--extras <JSON>`      |       | Extra parameters for the extractor (JSON string).  | (none)   |
| `--output <FORMAT>`    | `-o`  | Output format.                                     | `pretty` |
| `--output-file <PATH>` | `-O`  | Save the output to a file instead of `stdout`.     | (none)   |

---

### `platforms`

Lists all supported platforms and their URL patterns.

**Usage:** `strev platforms [OPTIONS]`

**Options:**

| Option       | Short | Description                               | Default |
| ------------ | ----- | ----------------------------------------- | ------- |
| `--detailed` | `-d`  | Show detailed information (currently no effect). | `false` |

---

### `config`

Manages the application configuration.

**Usage:** `strev config [OPTIONS]`

**Options:**

| Option    | Short | Description                       |
| --------- | ----- | --------------------------------- |
| `--show`  | `-s`  | Show the current configuration.   |
| `--reset` |       | Reset the configuration to defaults. |

---

### `completions`

Generates shell completion scripts.

**Usage:** `strev completions <SHELL>`

**Arguments:**

| Argument | Description                                       |
| -------- | ------------------------------------------------- |
| `SHELL`  | **Required.** The shell to generate completions for (e.g., `bash`, `zsh`, `fish`, `powershell`). |

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

## Supported Platforms

| Platform    | Supported URL Type                               | Description |
|-------------|--------------------------------------------------|-------------|
| Acfun       | `acfun.cn/live/{room_id}`                        | Live streaming platform |
| Bilibili    | `live.bilibili.com/{room_id}`                    | Live streaming platform |
| Douyin      | `live.douyin.com/{room_id}`                      | TikTok China live streaming |
| Douyu       | `douyu.com/{room_id}`                            | Gaming live streaming |
| Huya        | `huya.com/{room_id}`                             | Gaming live streaming |
| PandaTV     | `pandalive.co.kr/play/{user_id}` (Defunct)       | Live streaming platform |
| Picarto     | `picarto.tv/{channel_name}`                      | Art streaming platform |
| Redbook     | `xiaohongshu.com/user/profile/{user_id}` or `xhslink.com/{share_id}` | Lifestyle live streaming |
| TikTok      | `tiktok.com/@{username}/live`                   | Short-form video live streaming |
| TwitCasting | `twitcasting.tv/{username}`                      | Live broadcasting service |
| Twitch      | `twitch.tv/{channel_name}`                       | Gaming and creative content |
| Weibo       | `weibo.com/u/{user_id}` or `weibo.com/l/wblive/p/show/{live_id}` | Social media live streaming |

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
