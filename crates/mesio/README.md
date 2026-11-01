# Mesio Engine

A media downloader engine for Rust with HLS, MPEG-DASH and FLV support.

## Core API

`mesio-engine` starts downloads through `MesioDownloader` and returns a
per-download `DownloadSession<T>`:

- `items`: guaranteed media data and terminal/boundary markers.
- `events`: best-effort telemetry such as resource start, progress, finish, retry,
  and HLS playlist notices.
- `handle`: cancellation, dropped-event count, and optional metrics.

Use `start_hls`, `start_dash` or `start_flv` when the protocol is known. Use
`start` with `ProtocolSelection::Auto` when the URL should be detected at
runtime; a path ending in `.mpd` is detected as DASH.

## Session Contract

- `items` is lossless and backpressured. Boundaries travel on it, never on
  `events`: a normal end is `HlsData::EndMarker(Some(SplitReason::EndOfStream))`,
  a discontinuity is `HlsData::EndMarker(Some(SplitReason::Discontinuity))` (DASH
  also emits `StreamStructureChange` when the track layout changes), and a fatal
  failure is a terminal `Err(DownloadError)`; FLV ends by closing the stream.
  Pipe consumers can derive file/pipe boundaries from `items` alone.
- `events` is bounded and drops under backpressure (`handle.dropped_events()`
  counts drops). `Progress` is coalesced (by default every 256 KiB or 100 ms);
  `ResourceFinished` carries the exact byte count.
- `handle.metrics()` returns lossless HLS/DASH counters, or `None` when
  metrics are disabled (`metrics_enabled` in the HLS or DASH output config).
  Do not rebuild totals from `events`.
- `handle.join()` waits for the session's `DownloadTerminal`.
- With `DownloadRequest::sources`, HLS and DASH fail over by restarting on the
  next source, emitting `SourceSelected` and a `Discontinuity` end marker
  between attempts. FLV picks a source at startup only; non-default
  `FlvReconnect` modes are rejected with `DownloadError::Configuration`.

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
        mesio_engine::DownloaderSession::Hls(mut session)
        | mesio_engine::DownloaderSession::Dash(mut session) => {
            while let Some(item) = session.items.next().await {
                let item = item?;
                println!("fMP4/TS item: {item:?}");
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
`FlvProtocolBuilder`, `HlsProtocolBuilder` and `DashProtocolBuilder` build the
`FlvProtocolConfig`, `HlsConfig` and `DashConfig` it holds.

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

MPEG-DASH reuses that segment engine with one reactor and assembler per
selected track (video, audio), each with its own byte budget, fed by a single
MPD watcher. A muxer combines the tracks into one fragmented MP4 stream (one
item per video segment with the audio that overlaps it) and emits it as
`HlsData`, so DASH recordings are processed and written like HLS fMP4. Static
and dynamic MPDs with `SegmentTemplate` or `SegmentList` addressing, and
static MPDs with `SegmentBase` addressing, of unencrypted ISO BMFF content are
supported. The Segment Index (`sidx`) of each `SegmentBase` Representation the
track selection records is read when the MPD is fetched. DRM, WebM and live
`SegmentBase` are rejected with `DownloadError::Unsupported`.

FLV uses a streaming byte forwarder into the FLV decoder. All protocols expose
the same session and event contract.

HTTP failures while loading an initial or selected HLS playlist, or after live
playlist refresh retries are exhausted, retain `DownloadError::HttpStatus`.
Consumers can distinguish 401/403 for a bounded URL/account diagnostic without
parsing messages or treating every HTTP error as recoverable. Playlist retry
limits and cancellation still apply.

## Diagnostic privacy

The logging `Redacted` URL adapter shows only an origin and a redaction marker;
relative/malformed URIs are fully masked. `redact_url_for_log` provides the same
behavior for strings. `redact_url`/`redact_url_str` keep the URL structure
(userinfo removed, query keys kept, query values masked) for callers that need
it. Source-health, playlist and retry logs retain
counts, typed classes and numeric status information while omitting authentication
material, response bodies and free-form upstream failure details.

`Debug` for downloader configuration, content sources, source managers and cache
keys omits network material. Requests, source identities and cache keys still use
the original values. Consumers logging upstream errors can use `redact_diagnostic`
to keep provider/parser messages out of log sinks.
