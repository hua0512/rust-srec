# Siphon Engine

A modern, high-performance media streaming engine for Rust, supporting various streaming formats like HLS and FLV.

## Features

- **Multiple Protocol Support**: Ready-to-use implementations for HLS and FLV streaming
- **Capability-based Architecture**: Modular design allows selecting exact features needed
- **Caching System**: Efficient multi-level caching with memory and disk storage
- **Flexible Source Management**: Support for multiple content sources with automatic failover
- **Protocol Auto-detection**: Automatic protocol detection from stream URLs
- **Proxy Support**: Configurable proxy settings including system proxy detection
- **Async Design**: Built on Tokio for high-performance async I/O

## Usage Examples

### Basic Usage with Factory Pattern

```rust
use siphon_engine::{SiphonDownloaderFactory, ProtocolType, process_stream};
use tokio_stream::StreamExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create a factory for download managers
    let factory = SiphonDownloaderFactory::new();

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

```rust
use siphon_engine::{FlvProtocolBuilder, ProtocolBuilder, Download};
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

### HLS Streaming with Caching

```rust
use siphon_engine::{
    HlsProtocolBuilder, ProtocolBuilder, CacheConfig,
    DownloadManager, DownloadManagerConfig, Download
};
use std::time::Duration;
use tokio_stream::StreamExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Configure caching
    let cache_config = CacheConfig {
        enabled: true,
        playlist_ttl: Duration::from_secs(10),
        segment_ttl: Duration::from_secs(300),
        ..CacheConfig::default()
    };

    // Create HLS protocol with high-quality selection
    let hls = HlsProtocolBuilder::new()
        .select_highest_quality(true)
        .max_concurrent_segment_downloads(4)
        .segment_retry_count(3)
        .build()?;

    // Create a download manager with caching
    let mut manager = DownloadManager::with_config(
        hls,
        DownloadManagerConfig {
            cache_config: Some(cache_config),
            ..DownloadManagerConfig::default()
        }
    ).await?;

    // Stream HLS content
    let stream = manager.download("https://example.com/playlist.m3u8").await?;

    // Process the stream
    tokio::pin!(stream);
    while let Some(result) = stream.next().await {
        // Process HLS data...
    }

    Ok(())
}
```

### Multi-Source Download with Fallback

```rust
use siphon_engine::{SiphonDownloaderFactory, ProtocolType};
use tokio_stream::StreamExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create a factory
    let factory = SiphonDownloaderFactory::new();

    // Create a downloader
    let mut downloader = factory
        .create_for_url("https://example.com/video.flv", ProtocolType::Flv)
        .await?;

    // Add multiple sources with priorities
    downloader.add_source("https://mirror1.example.com/video.flv", 1);
    downloader.add_source("https://mirror2.example.com/video.flv", 2);

    // Download with automatic fallback
    let stream = downloader.download_with_sources("https://example.com/video.flv").await?;

    // Process the stream...

    Ok(())
}
```

## Advanced Configuration

### Creating a Factory with Custom Settings

```rust
use siphon_engine::{
    SiphonDownloaderFactory,
    DownloadManagerConfig,
    FlvConfig,
    HlsConfig,
    ProxyConfig,
    ProxyType,
    ProxyAuth
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
let factory = SiphonDownloaderFactory::new()
    .with_download_config(download_config)
    .with_flv_config(flv_config)
    .with_hls_config(hls_config);

// Now use the factory to create protocol-specific downloader instances
```

## Component Architecture

The library is built around these key components:

- **Protocol Handlers**: Implementations for specific formats (HLS, FLV)
- **Download Manager**: Coordinates sources and manages capabilities
- **Cache System**: Multi-level caching with memory and disk backends
- **Source Manager**: Handles multiple content sources with failover
- **Factory**: Creates appropriate downloaders with protocol auto-detection

## License

Licensed under either:

- MIT License
- Apache License, Version 2.0

at your option.
