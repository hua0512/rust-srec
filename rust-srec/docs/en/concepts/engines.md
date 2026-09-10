# Engines

Downloaders are the core of the application. They are responsible for downloading the video stream from the source. The application supports three downloaders: `Mesio`, `FFMPEG`, and `Streamlink`. Each downloader has its own features and limitations.

`Streamlink` appears on two independent axes and the names are easy to conflate. As a *downloader* it is the process that writes the recording, chosen with `download_engine` and described on this page. As an *extractor* it is what resolves the stream URL before any downloader runs, chosen with `extractor`; see [Engine and extractor selection](./configuration.md#engine-and-extractor-selection). Either can be set without the other.

> [!TIP]
> For **Mesio** users, it is **highly recommended** to enable both **FLV Consistency Fix** and **HLS Consistency Fix**. These pipelines correct or isolate timestamp and stream-structure changes so one bad transition is less likely to make the rest of a recording undecodable. Media that the source never delivered cannot be recovered.

The `FFMPEG` downloader is the compatibility-focused external engine. It is written in C and can download FLV and HLS streams, including non-standard HEVC in FLV/RTMP containers. It does not support multithreaded HLS downloads. Fresh installations select the built-in Mesio engine instance by default; choose FFmpeg explicitly when its container or codec compatibility is required.

> [!NOTE]
> The FFMPEG version provided in our Docker images is a specialized build from [yt-dlp/FFmpeg-Builds](https://github.com/yt-dlp/FFmpeg-Builds/). This build is optimized for streaming and includes (or has upstreamed) critical patches for smooth integration with `yt-dlp`, such as fixing AAC HLS truncation, supporting long paths on Windows, and decoding non-standard HEVC in FLV containers.

### Recording Filenames and Telemetry

Recording templates expand their configured date tokens, while percent signs in
streamer names and titles remain literal. For example, a title `Top 5%d` stays
`Top 5%d`; it does not substitute the day of the month. Startup output-root probes
use the same literal metadata rules.

FFmpeg recording and Streamlink remuxing require info-level logs and statistics
to track segments and progress. The backend supplies final `-loglevel info
-stats` options; custom quiet log settings and `-nostats` do not disable this
telemetry.

### Executable Checks

FFmpeg and Streamlink version checks have a three-second deadline, followed by bounded process cleanup if needed. Engine tests and custom-engine resolution run these checks asynchronously. An executable that cannot be started or does not finish its version check before the deadline is reported unavailable. Synchronous startup initialization waits for the same bounded check.

Missing named engine configurations retain the default-engine fallback. Database access failures, malformed saved settings, and invalid overrides fail resolution instead of silently changing the effective configuration.

### Streamlink FFmpeg Executable

The Streamlink download engine accepts an optional `ffmpeg_path` in its backend
engine configuration JSON. `binary_path` selects Streamlink itself; `ffmpeg_path`
selects the separate FFmpeg process that remuxes its output:

```json
{
  "binary_path": "streamlink",
  "ffmpeg_path": "/opt/media-tools/ffmpeg",
  "quality": "best"
}
```

The backend selects the configured `ffmpeg_path` first, then the `FFMPEG_PATH`
environment variable, then `ffmpeg` from `PATH`. Omit the field or set it to `null`
to retain the environment/default behavior. Paths containing spaces are passed
as one executable path, without shell quoting or arguments. The choice is made
when the engine instance is constructed; an environment change requires a backend
restart. An explicit path does not inherit settings from any registered FFmpeg
engine, and a missing executable fails recording startup instead of falling back.
Empty or whitespace-only strings are explicit executable values, not a request
for fallback; use omission or `null` to clear the override.

### Stopping Streamlink Recordings

For an audited Streamlink 8.5.0 installation, stopping a recording requests an
acquisition stop through a private authenticated control connection. The configured
CLI, platform plugins, authentication, proxy settings, quality and extra arguments
remain in use. A bounded loader probe establishes support before the backend adds
its embedded companion; unchanged executable probes are coalesced and cached.
Each recording still verifies the companion and its actual reader graph.

| Reader profile | Cooperative drain behavior |
| --- | --- |
| Segmented HLS/DASH, including filtering and nested muxers | Stop new acquisition, finish admitted segment/map work, and drain reader buffers and every muxer input to EOF. |
| HTTP on CPython 3.11/3.14 with requests 2.34.2 and urllib3 2.7.0 | Drain HTTP prefetch, decrypted TLS pending bytes, and identity/gzip/deflate decoder buffers. Opaque transports and other decoders are not included. |
| TwitCasting websocket with websocket-client 1.9.2 | Finish an admitted receive and its delivery before stopping the next message. |
| File paths and in-memory BytesIO input | Preserve the current unbuffered file read or all already-acquired in-memory bytes. Arbitrary buffered file objects are not included. |

The companion checks upstream source compatibility rather than trusting version
text alone. Unsupported old, portable or custom executables retain ordinary
recording behavior. Unsupported reader graphs also retain their CLI behavior,
but a requested stop reports `Streamlink cooperative drain incomplete` instead
of claiming a verified drain. No recording engine feature is disabled to establish
compatibility.

Stdout forwarding and stderr processing remain alive while producers stop. A
successful cooperative stop requires actual source EOF, complete pipe forwarding
and successful external FFmpeg finalization; a control acknowledgement alone is
insufficient. Hidden Windows processes use the control connection without console
signals. Internal FFmpeg muxers and validation processes stay hidden, and Windows
muxer pipes flush accepted bytes before disconnecting.

The attempt's remaining stop deadline bounds these process, forwarding and remux
phases, with time reserved for FFmpeg finalization. A later shutdown can tighten
an already-stopping attempt. Required final-event delivery remains owned until its
consumer accepts it: direct engine integrations must drain the event channel,
and a stalled consumer is bounded by the worker's overall force cap. Deadline
expiry still forces contained process-tree termination and can truncate the tail.
The guarantee covers acquired data and admitted work, not future undiscovered
segments, upstream corruption or work that exceeds the deadline.

Both subprocess trees remain contained, including descendants left by an exiting
parent. On macOS, leader-exit races use the remaining containment budget; they do
not introduce another grace period. This assumes descendants do not deliberately
escape the process group or Windows Job Object.

## 1. Engines Feature List

|         Feature          |                Mesio                 |                 FFMPEG                  |               STREAMLINK                |
| :----------------------: | :-----------------------------------: | :-------------------------------------: | :-------------------------------------: |
|       FLV Download       |                  ✅                   |                   ✅                    |                   ❌                    |
|       HLS Download       |        ✅ <br/>(Multithreaded)        |                   ✅                    |         ✅ <br/>(Multithreaded)         |
| Recording Duration Stats | ✅ <br/>(Raw data mode not supported) |                   ✅                    |                   ✅                    |
|  Download Bitrate Stats  |                  ✅                   | ✅ <br/>(-f segmentation not supported) | ✅ <br/>(-f segmentation not supported) |
|    Size Segmentation     | ✅ <br/>(Raw data mode not supported) | ✅ <br/>(-f segmentation not supported) |                   ✅                    |
|  Duration Segmentation   | ✅ <br/>(Raw data mode not supported) |                   ✅                    |                   ✅                    |
|     Download Format      |           FLV, M3U8, TS, M4S          |         Supports other formats          |         Supports other formats          |
|    FLV Consistency Fix   |           ✅ (Recommended)            |                   ❌                    |                   ❌                    |
|    HLS Consistency Fix   |           ✅ (Recommended)            |                   ❌                    |                   ❌                    |
|        CPU Usage         |                  Lowest               |                   Low                   |                   Low                   |
|       Memory Usage       |                  Lowest               |                   Low                   |                 Medium                  |

## 2. FLV Consistency Fix

When enabled, FLV items pass through one ordered repair chain before the writer. The chain preserves media payloads while fixing container-level structure:

| Concern | Pipeline behavior |
| --- | --- |
| Missing FLV header | Insert a valid header before the first tag |
| Video or audio format change | Start a new output file with the headers required for playback |
| Out-of-order video data | Reorder nearby tags while keeping memory use bounded |
| Timestamp jumps or regressions | Adjust timestamps to preserve playback continuity |
| Duplicate media tags or sequence headers | Filter them when the corresponding option is enabled |
| File size or duration limit | Rotate on a video keyframe, or on the next tag for audio-only output |
| FLV statistics and seek index | Update metadata before the file is closed |

Metadata updates do not rewrite the completed media data. If the available metadata space fills, the optional seek index is shortened instead. Filtered or encrypted script payloads are left unchanged.

## 3. Raw Data Mode

Raw Data Mode is a high-performance download mode supported by the **Mesio** engine. In this mode, the engine writes the stream data directly to the disk as it's received from the network, without parsing or processing the internal media packets (headers, frames, metadata).

### Key Characteristics:
- **Maximum Performance**: Since there is no packet parsing or re-muxing, CPU and memory usage are at their absolute minimum.
- **Zero Overhead**: Ideal for high-bandwidth streams or resource-constrained environments (like low-end NAS or VPS). Only recommended if the network is stable and the CDN/stream source has no data fluctuations (e.g., media headers changing).

### Limitations:
Because the headers and packet structures are not inspected, some advanced features are unavailable when Raw Data Mode is enabled:
- **Statistics**: Recording duration and bitrate stats cannot be calculated in real-time.
- **Segmentation**: The engine cannot detect frame boundaries or duration, so it cannot perform precise segmentation by size or duration.
- **Repair**: Features like FLV Consistency Fix or HLS Consistency Fix cannot be applied as they require packet-level manipulation.

## 4. HLS Consistency Fix (Mesio Exclusive)

Mesio's HLS download reactor and HLS fix pipeline have separate responsibilities:

- The **download engine** orders fetched segments, writes required fMP4 initialization data before dependent media, applies the configured gap policy, and preserves playlist discontinuities as output boundaries.
- The **consistency fix** starts a new output file when delivered segments contain incompatible codec, resolution, program-layout, or fMP4 initialization changes. It also enforces file-size and duration limits.

The pipeline does not rewrite timestamps inside TS or fMP4 payloads, recreate missing media, or transcode codecs. A skipped segment remains an observable gap; the pipeline keeps delivered output ordered and rotates when a detected format change requires a new file.

## 5. Mesio Architecture

Mesio is an **in-process Rust engine** with a reactor-based HLS downloader and a unified download-session model shared by HLS and FLV. For the architecture diagram and a walkthrough of how it works under the hood, see the dedicated [Mesio Engine](./mesio.md) page.
