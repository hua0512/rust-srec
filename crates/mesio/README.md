# Mesio Engine

[![License](https://img.shields.io/crates/l/mesio-engine.svg)](https://github.com/hua0512/rust-srec)

A modern, high-performance media streaming engine for Rust, supporting various streaming formats like HLS and FLV.


## Core Concepts

`mesio-engine` is built around a few key concepts:

- **`DownloadManager`**: The central component that coordinates the download process. It manages capabilities like caching, multi-source fallback, and proxy support.
- **`MesioDownloaderFactory`**: A factory for creating `DownloadManager` instances. It can automatically detect the protocol from a URL and configure the appropriate downloader (HLS or FLV).
- **Capability-based Traits**: The library uses a system of traits to define the capabilities of a protocol downloader. These include:
    - `Download`: Basic download functionality.
    - `Resumable`: Support for resuming downloads.
    - `MultiSource`: Ability to handle multiple download sources with fallback.
    - `Cacheable`: Caching support for playlists and segments.

## Usage Examples

### Basic Usage with Factory Pattern

The easiest way to use `mesio-engine` is with the `MesioDownloaderFactory`. It automatically detects the protocol and creates a configured downloader.

```rust
use mesio_engine::{MesioDownloaderFactory, ProtocolType, process_stream};
use tokio_stream::StreamExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create a factory for download managers
    let factory = MesioDownloaderFactory::new();

    // Create a downloader with automatic protocol detection
    let mut downloader = factory
        .create_for_url("https://example.com/video.m3u8", ProtocolType::Auto)
        .await?;

    // Download the stream
    let stream = downloader.download("https://example.com/video.m3u8").await?;

    // Process the stream with type-specific handling
    tokio::pin!(stream);
    while let Some(data) = stream.next().await {
        process_stream!(stream, {
            flv(flv_stream) => {
                // Handle FLV-specific data
                println!("Processing FLV data: {:?}", flv_stream);
            },
            hls(hls_stream) => {
                // Handle HLS-specific data
                println!("Processing HLS data: {:?}", hls_stream);
            },
        });
    }

    Ok(())
}
```

### Protocol-Specific Approach with Builders

For more control, you can use protocol-specific builders to create and configure downloaders.

```rust
use mesio_engine::{FlvProtocolBuilder, ProtocolBuilder, Download};
use tokio_stream::StreamExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create an FLV protocol handler
    let flv = FlvProtocolBuilder::new()
        .buffer_size(128 * 1024) // Use 128KB buffer
        .build()?;

    // Download an FLV stream
    let stream = flv.download("https://example.com/video.flv").await?;

    // Process the stream
    tokio::pin!(stream);
    while let Some(result) = stream.next().await {
        match result {
            Ok(data) => {
                // Process FLV data packet
                println!("Received FLV packet: {:?}", data.tag_type());
            },
            Err(e) => eprintln!("Error: {}", e),
        }
    }

    Ok(())
}
```

## Configuration

You can customize the behavior of the `DownloadManager` and protocol-specific handlers using builder patterns.

### Advanced `DownloadManager` Configuration

```rust
use mesio_engine::{DownloadManager, HlsProtocolBuilder, DownloadManagerConfig, CacheConfig};
use std::time::Duration;

# #[tokio::main]
# async fn main() -> Result<(), Box<dyn std::error::Error>> {
// Configure caching
let cache_config = CacheConfig {
    enabled: true,
    playlist_ttl: Duration::from_secs(10),
    segment_ttl: Duration::from_secs(300),
    ..CacheConfig::default()
};

// Create a download manager with caching
let hls_protocol = HlsProtocolBuilder::new().build()?;
let mut manager = DownloadManager::with_config(
    hls_protocol,
    DownloadManagerConfig {
        cache_config: Some(cache_config),
        ..DownloadManagerConfig::default()
    }
).await?;
# Ok(())
# }
```

### Creating a Factory with Custom Settings

You can also pre-configure a `MesioDownloaderFactory` with custom settings that will be applied to all downloaders it creates.

```rust
use mesio_engine::{
    MesioDownloaderFactory,
    DownloadManagerConfig,
    config::{FlvConfig, HlsConfig},
    proxy::{ProxyConfig, ProxyType, ProxyAuth}
};

// Create proxy configuration
let proxy_config = ProxyConfig {
    url: "socks5://proxy.example.com:1080".to_string(),
    proxy_type: ProxyType::Socks5,
    auth: Some(ProxyAuth {
        username: "user".to_string(),
        password: "pass".to_string(),
    }),
};

// Configure download manager
let download_config = DownloadManagerConfig {
    proxy: Some(proxy_config),
    use_system_proxy: false,
    // Other settings...
    ..DownloadManagerConfig::default()
};

// Configure protocol-specific settings
let flv_config = FlvConfig {
    buffer_size: 64 * 1024,
    // Other FLV settings...
    ..FlvConfig::default()
};

let hls_config = HlsConfig {
    select_highest_quality: true,
    max_concurrent_downloads: 4,
    // Other HLS settings...
    ..HlsConfig::default()
};

// Create factory with all settings configured
let factory = MesioDownloaderFactory::new()
    .with_download_config(download_config)
    .with_flv_config(flv_config)
    .with_hls_config(hls_config);

// Now use the factory to create protocol-specific downloader instances
```

## Component Architecture

The library is built around these key components:

- **Protocol Handlers**: Implementations for specific formats (HLS, FLV) that provide the core download capabilities.
- **`DownloadManager`**: Coordinates sources and manages capabilities like caching and proxies.
- **Cache System**: Multi-level caching with memory and disk backends.
- **`SourceManager`**: Handles multiple content sources with failover.
- **`MesioDownloaderFactory`**: Creates and configures appropriate downloaders with protocol auto-detection.

## License

Licensed under either:

- MIT License
- Apache License, Version 2.0

at your option.
