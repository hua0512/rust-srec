# Siphon CLI

[![GitHub](https://img.shields.io/badge/github-hua0512/rust--srec-8da0cb?logo=github)](https://github.com/hua0512/rust-srec)
![Version](https://img.shields.io/badge/version-0.2.3-blue)
![Rust](https://img.shields.io/badge/rust-2024-orange)

Siphon is a powerful command-line tool for downloading, processing, and repairing FLV (Flash Video) streams and files. It's part of the [rust-srec](https://github.com/hua0512/rust-srec) project, designed to handle common issues in FLV streams.

## Features

- Download FLV streams directly from URLs
- Process and repair existing FLV files
- Fix common issues such as:
  - Timestamp anomalies
  - Out-of-order frames
  - Duration problems
  - Metadata inconsistencies
- Advanced proxy support (HTTP, HTTPS, SOCKS5)
- File segmentation by size or duration
- Progress reporting with customizable progress bars
- Keyframe indexing for better seeking

## Installation

### From Source

To build and install from source, you need Rust and Cargo installed on your system.

```bash
# Clone the repository
git clone https://github.com/hua0512/rust-srec.git
cd rust-srec

# Build and install the siphon CLI tool
cargo build --release -p siphon

# The binary will be available at
./target/release/siphon
```

### Pre-built Binaries

Check the [releases page](https://github.com/hua0512/rust-srec/releases) for pre-built binaries for your platform.

## Basic Usage

```bash
# Download an FLV stream from a URL
siphon --progress https://example.com/stream.flv

# Process an existing FLV file
siphon --progress --fix path/to/file.flv

# Download with custom output directory
siphon --progress -o downloads/ https://example.com/stream.flv

# Process multiple inputs
siphon --progress --fix file1.flv file2.flv https://example.com/stream.flv
```

## Command-Line Options

### Input/Output Options

```
REQUIRED:
  <INPUT>...                Path to FLV file(s), directory containing FLV files, or URL(s) to download

OPTIONS:
  -o, --output-dir <DIR>    Directory where processed files will be saved (default: ./fix)
  -n, --name <TEMPLATE>     Output file name template (e.g., '%Y%m%d_%H%M%S_p%i')
```

### Processing Options

```
  -m, --max-size <SIZE>     Maximum size for output files with unit (B, KB, MB, GB, TB)
                            Examples: "4GB", "500MB". Use 0 for unlimited.
  -d, --max-duration <DUR>  Maximum duration for output files with unit (s, m, h)
                            Examples: "30m", "1.5h", "90s". Use 0 for unlimited.
  -k, --keyframe-index      Inject keyframe index in metadata for better seeking [default: true]
      --fix                 Enable processing/fixing pipeline (by default streams are downloaded as raw data)
  -b, --buffer-size <SIZE>  Buffer size for internal processing channels [default: 16]
      --download-buffer <SIZE>  Buffer size for downloading in bytes [default: 65536]
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
siphon --progress -m 500MB -d 30m https://example.com/stream.flv
```

### Custom Output Names

Use a template for output filenames:

```bash
siphon --progress --name "stream_%Y%m%d_%H%M%S" https://example.com/stream.flv
```

### Using a Proxy

Download through an HTTP proxy:

```bash
siphon --progress --proxy "http://proxy.example.com:8080" --proxy-type http https://example.com/stream.flv
```

With authentication:

```bash
siphon --progress --proxy "http://proxy.example.com:8080" --proxy-user username --proxy-pass password https://example.com/stream.flv
```

### Custom HTTP Headers

Add custom HTTP headers for the request:

```bash
siphon --progress -H "Referer: https://example.com" -H "User-Agent: Custom/1.0" https://example.com/stream.flv
```

### Process and Fix Existing Files

Enable the processing pipeline to repair files:

```bash
siphon --progress --fix file.flv
```

## Advanced Usage

### Keyframe Indexing

For better seeking in media players, enable keyframe indexing:

```bash
siphon --progress --fix --keyframe-index file.flv
```

### Setting Timeouts

Configure network timeouts for unstable connections:

```bash
siphon --progress --connect-timeout 60 --read-timeout 45 https://example.com/stream.flv
```

## License

This project is part of the [rust-srec](https://github.com/hua0512/rust-srec) project.

## Credits

Developed by [hua0512](https://github.com/hua0512).