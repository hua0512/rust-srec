# Strev CLI

A high-performance, user-friendly CLI tool for extracting streaming media information from various online platforms.

## Features

- **Extraction** of stream information through `platforms-parser`, with a Streamlink fallback for URLs no built-in extractor handles.
- **Retries with exponential backoff** and a per-request timeout.
- **Batch processing** of URLs from a file, concurrently.
- **Output formats**: pretty, JSON, compact JSON, table, CSV.
- **Stream selection**: interactive prompt, or automatic with `--auto-select`; filter by quality and stream format.
- **Proxy support**: HTTP, HTTPS and SOCKS5, with authentication.
- **Shell completions** for bash, zsh, fish, PowerShell and elvish.

Cargo features (all default): `interactive` (stream selection prompt), `colored-output`,
`table-output`, `regex-filters` (`--quality`/`--format` are regular expressions; substring
matches without it).

## Installation

```bash
cargo build --locked --release -p strev
# The binary is ./target/release/strev
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
| `--format <FORMAT>`    |       | Filter streams by stream format (`flv`, `hls`, `mp4`, `wss`, `dash`). | (none)   |
| `--auto-select`        |       | Auto-select the best stream without a prompt.      | `false`  |

#### Behavior

*   **Stream selection:** Only the `pretty` and `table` output formats select a stream and resolve its final URL; the other formats print the extracted media information. When several streams match the filters and `--auto-select` is not used, a prompt asks for one.
*   **Auto-Selection:** `--auto-select` picks the stream with the lowest priority value, then the highest bitrate.
*   Some streams (for example DASH) are offered only on request: they are never auto-selected unless `--format` names their format.

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
| `--continue-on-error`     |       | Accepted but has no effect: failed URLs are always reported and processing continues. | `false` |

#### Behavior

*   Reads URLs from the specified input file, ignoring empty lines and lines starting with `#`.
*   Always uses **auto-selection** (lowest priority value, then highest bitrate); streams offered only on request are skipped.
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
| `--detailed` | `-d`  | Accepted but has no effect.               | `false` |

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
| `SHELL`  | **Required.** The shell to generate completions for (`bash`, `elvish`, `fish`, `powershell`, `zsh`). |

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

These keys go in the configuration file described below; `--proxy` overrides them.

```toml
default_proxy = "http://proxy.example.com:8080"
default_proxy_username = "username"
default_proxy_password = "password"
```

## Configuration File

The configuration file is TOML. Without `--config`, it is read from
`<config dir>/streev-cli/default-config.toml`, where `<config dir>` is:

- **Windows**: `%APPDATA%`
- **macOS**: `~/Library/Application Support`
- **Linux**: `$XDG_CONFIG_HOME` or `~/.config`

A missing file means defaults. Every non-optional key must be present, so start from
`strev config --reset`, which writes the defaults to that path (or to `--config`), and edit the
result; `strev config --show` prints the effective configuration.

```toml
default_output_format = "pretty"
default_timeout = 30
default_retries = 3
max_concurrent = 5
auto_select = false
include_extras = true
colored_output = true
# Optional: default_cookies, default_output_dir, user_agent,
# default_proxy, default_proxy_username, default_proxy_password
```

Only `colored_output` and the `default_proxy*` settings take effect; the other commands use
their command-line options and defaults.

## Supported Platforms

Run `strev platforms` for the list built into the CLI. The extractor accepts these URLs (the
`www.` prefix is optional):

| Platform    | Supported URL Type                               | Description |
|-------------|--------------------------------------------------|-------------|
| Acfun       | `acfun.cn/live/{room_id}`                        | Live streaming platform |
| Bigo        | `bigo.tv/{room_id}`                              | Live streaming platform |
| Bilibili    | `live.bilibili.com/{room_id}`                    | Live streaming platform |
| Douyin      | `live.douyin.com/{room_id}`                      | TikTok China live streaming |
| Douyu       | `douyu.com/{room_id}`                            | Gaming live streaming |
| Huya        | `huya.com/{room_id}`                             | Gaming live streaming |
| PandaTV     | `pandalive.co.kr/play/{user_id}`                 | Live streaming platform |
| Picarto     | `picarto.tv/{channel_name}`                      | Art streaming platform |
| Redbook     | `xhslink.com/m/{share_id}` or `xhslink.com/o/{share_id}` | Lifestyle live streaming |
| SOOP        | `play.sooplive.co.kr/{user_id}`                  | Live streaming platform (formerly AfreecaTV) |
| TikTok      | `tiktok.com/@{username}/live`                    | Short-form video live streaming |
| TwitCasting | `twitcasting.tv/{username}`                      | Live broadcasting service |
| Twitch      | `twitch.tv/{channel_name}`                       | Gaming and creative content |
| Weibo       | `weibo.com/u/{user_id}` or `weibo.com/l/wblive/p/show/{live_id}` | Social media live streaming |

Other URLs are passed to Streamlink when the `streamlink` executable is installed. See
[`crates/platforms/README.md`](../crates/platforms/README.md) for host variants.

## Development

```bash
cargo build --locked -p strev
cargo nextest run --locked -p strev
```

## License

This project is licensed under the MIT OR Apache-2.0 license.
