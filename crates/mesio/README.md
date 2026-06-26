# Mesio Engine

A media downloader engine for Rust with HLS and FLV support.

## Core API

`mesio-engine` starts downloads through `MesioDownloader` and returns a
per-download `DownloadSession<T>`:

- `items`: guaranteed media data and terminal/boundary markers.
- `events`: best-effort telemetry such as resource start, progress, finish, retry,
  and HLS playlist notices.
- `handle`: cancellation, dropped-event count, and optional metrics.

Use `start_hls` or `start_flv` when the protocol is known. Use `start` with
`ProtocolSelection::Auto` when the URL should be detected at runtime.

## Basic Usage

```rust
use futures::StreamExt;
use mesio_engine::{DownloadRequest, MesioConfig, MesioDownloader, ProtocolSelection};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let downloader = MesioDownloader::new(MesioConfig::default());

    let request = DownloadRequest::from_url("https://example.com/video.m3u8")?
        .with_protocol(ProtocolSelection::Auto);

    let session = downloader.start(request).await?;

    match session {
        mesio_engine::DownloaderSession::Hls(mut session) => {
            while let Some(item) = session.items.next().await {
                let item = item?;
                println!("HLS item: {item:?}");
            }
        }
        mesio_engine::DownloaderSession::Flv(mut session) => {
            while let Some(item) = session.items.next().await {
                let item = item?;
                println!("FLV item: {item:?}");
            }
        }
    }

    Ok(())
}
```

## Progress Events

The event stream is optional to consume and safe to drop. Event delivery is
bounded and best effort, so progress rendering cannot stall media reads.

```rust
use futures::StreamExt;
use mesio_engine::{
    DownloadEvent, DownloadRequest, MesioConfig, MesioDownloader, ProtocolSelection,
};

# async fn example() -> Result<(), Box<dyn std::error::Error>> {
let downloader = MesioDownloader::new(MesioConfig::default());
let request = DownloadRequest::from_url("https://example.com/video.flv")?
    .with_protocol(ProtocolSelection::Flv(Default::default()));

let mut session = downloader.start_flv(request).await?;
let mut events = session.events;

tokio::spawn(async move {
    while let Some(event) = events.next().await {
        match event {
            DownloadEvent::Progress { bytes_delta, bytes_total, .. } => {
                println!("downloaded +{bytes_delta} bytes ({bytes_total} total)");
            }
            DownloadEvent::ResourceFinished { bytes, .. } => {
                println!("resource finished: {bytes} bytes");
            }
            _ => {}
        }
    }
});

while let Some(item) = session.items.next().await {
    let _item = item?;
}
# Ok(())
# }
```

## Configuration

Use `MesioConfig` for protocol configuration and cancellation defaults.
Protocol-specific builders remain available for constructing `HlsConfig` and
`FlvProtocolConfig`.

```rust
use mesio_engine::{FlvProtocolBuilder, HlsProtocolBuilder, MesioConfig, MesioDownloader};

let flv = FlvProtocolBuilder::new()
    .buffer_size(128 * 1024)
    .get_config();

let hls = HlsProtocolBuilder::new()
    .download_concurrency(8)
    .get_config();

let downloader = MesioDownloader::new(MesioConfig {
    flv,
    hls,
    ..MesioConfig::default()
});
```

## Architecture

HLS uses a reactor-based lifecycle internally:

- `PlaylistWatcher` polls playlists.
- The scheduler reactor owns segment state, deduplication, retries, and bounded
  fetch task spawning.
- Fetch tasks own network I/O, byte budgets, and crypto processing.
- `SequenceAssembler` emits ordered media items and authoritative terminal
  markers.

FLV uses a streaming byte forwarder into the FLV decoder. Both protocols expose
the same session and event contract.
