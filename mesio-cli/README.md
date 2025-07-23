# Mesio CLI

[![crates.io](https://img.shields.io/crates/v/mesio-cli.svg)](https://crates.io/crates/mesio-cli)
![Version](https://img.shields.io/badge/version-0.2.4-blue)![Rust](https://img.shields.io/badge/rust-2024-orange)

Mesio is a powerful command-line tool for downloading, processing, and repairing media streams and files, with support for both **FLV (Flash Video)** and **HLS (HTTP Live Streaming)**. It's part of the [rust-srec](https://github.com/hua0512/rust-srec) project, designed to handle common issues in media streams.

## Features

- **Multi-Protocol Support**: Download and process both **FLV** and **HLS** streams.
- **Stream Repair**: Fix common issues in FLV streams such as:
  - Timestamp anomalies
  - Out-of-order frames
  - Duration problems
  - Metadata inconsistencies
- **HLS Downloader**:
  - Concurrent segment downloads for faster performance.
  - Automatic retries for failed segments.
  - Playlist caching to reduce redundant requests.
- **Advanced Proxy Support**: HTTP, HTTPS, and SOCKS5 proxies for all downloads.
- **File Segmentation**: Split output files by size or duration.
- **Customizable Output**: Use templates for file naming.
- **Progress Reporting**: Detailed progress bars for downloads and processing.
- **Keyframe Indexing**: Inject keyframe data into FLV files for better seeking.

## Installation

### From Source

To build and install from source, you need Rust and Cargo installed on your system.

```bash
# Clone the repository
git clone https://github.com/hua0512/rust-srec.git
cd rust-srec

# Build and install the mesio CLI tool
cargo build --release -p mesio

# The binary will be available at
./target/release/mesio
```

### Pre-built Binaries

Check the [releases page](https://github.com/hua0512/rust-srec/releases) for pre-built binaries for your platform.

## Basic Usage

```bash
# Download an FLV stream from a URL
mesio --progress https://example.com/stream.flv

# Download an HLS stream from a URL
mesio --progress https://example.com/playlist.m3u8

# Process an existing FLV file
mesio --progress --fix path/to/file.flv

# Download with custom output directory
mesio --progress -o downloads/ https://example.com/stream.flv

# Process multiple inputs (FLV, HLS, local files)
mesio --progress --fix file1.flv https://example.com/playlist.m3u8
```

## Command-Line Options

### Input/Output Options

```
REQUIRED:
  <INPUT>...                Path to media file(s), directory, or URL(s) to download

OPTIONS:
  -o, --output-dir <DIR>    Directory where processed files will be saved (default: ./fix)
  -n, --name <TEMPLATE>     Output file name template (e.g., '%u%Y%m%d_%H%M%S_p%i')
      --output-format <FORMAT>  Output format for downloaded content [default: file, values: file, stdout, stderr]
```

### Processing Options

```
  -m, --max-size <SIZE>     Maximum size for output files (e.g., "4GB", "500MB"). Use 0 for unlimited.
  -d, --max-duration <DUR>  Maximum duration for output files (e.g., "30m", "1.5h"). Use 0 for unlimited.
  -k, --keyframe-index      Inject keyframe index in metadata for better seeking [default: true]
      --fix                 Enable processing/fixing pipeline (by default streams are downloaded as raw data)
  -b, --buffer-size <SIZE>  Buffer size for internal processing channels [default: 16]
      --download-buffer <SIZE>  Buffer size for downloading in bytes [default: 65536]
```

### HLS Options

```
      --hls-concurrency <NUM>     Maximum number of concurrent segment downloads [default: 4]
      --hls-retries <NUM>         Number of retry attempts for failed segments [default: 3]
      --hls-segment-timeout <SEC> Timeout for individual segment downloads in seconds [default: 30]
      --hls-cache-playlists     Enable caching of HLS playlists [default: true]
```

### Network Options

```
      --timeout <SECONDS>          Overall timeout in seconds for HTTP requests [default: 0]
      --connect-timeout <SECONDS>  Connection timeout in seconds [default: 30]
      --read-timeout <SECONDS>     Read timeout in seconds [default: 30]
      --write-timeout <SECONDS>    Write timeout in seconds [default: 30]
  -H, --header <HEADER>            Add custom HTTP header (can be used multiple times). Format: 'Name: Value'
```

### Proxy Options

```
      --proxy <URL>              Proxy server URL (e.g., "http://proxy.example.com:8080")
      --proxy-type <TYPE>        Proxy type: http, https, socks5, all [default: http]
      --proxy-user <USERNAME>    Username for proxy authentication
      --proxy-pass <PASSWORD>    Password for proxy authentication
      --use-system-proxy         Use system proxy settings if no explicit proxy is configured [default: true]
      --no-proxy                 Disable all proxy settings for downloads
```

### Display Options

```
  -P, --progress         Show progress bars for download and processing operations
  -v, --verbose          Enable detailed debug logging
  -h, --help             Print help
  -V, --version          Print version
```

## Examples

### Download with Size and Duration Limits

Split the output into multiple files, limiting each to 500MB and 30 minutes:

```bash
mesio --progress -m 500MB -d 30m https://example.com/stream.flv
```

### Download an HLS Stream with High Concurrency

Download an HLS stream using 8 concurrent segment downloads:

```bash
mesio --progress --hls-concurrency 8 https://example.com/playlist.m3u8
```

### Custom Output Names

Use a template for output filenames:

```bash
mesio --progress --name "stream_%Y%m%d_%H%M%S" https://example.com/stream.flv
```

### Using a Proxy

Download through an HTTP proxy:

```bash
mesio --progress --proxy "http://proxy.example.com:8080" --proxy-type http https://example.com/stream.flv
```

### Custom HTTP Headers

Add custom HTTP headers for the request:

```bash
mesio --progress -H "Referer: https://example.com" -H "User-Agent: Custom/1.0" https://example.com/stream.flv
```

### Process and Fix Existing FLV Files

Enable the processing pipeline to repair FLV files:

```bash
mesio --progress --fix file.flv
```

## License

This project is part of the [rust-srec](https://github.com/hua0512/rust-srec) project and is licensed under the MIT OR Apache-2.0 license.

## Credits

Developed by [hua0512](https://github.com/hua0512).