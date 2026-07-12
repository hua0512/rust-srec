# Engines

Downloaders are the core of the application. They are responsible for downloading the video stream from the source. The application supports three downloaders: `Mesio`, `FFMPEG`, and `Streamlink`. Each downloader has its own features and limitations.

> [!TIP]
> For **Mesio** users, it is **highly recommended** to enable both **FLV Consistency Fix** and **HLS Consistency Fix**. These pipelines correct or isolate timestamp and stream-structure changes so one bad transition is less likely to make the rest of a recording undecodable. Media that the source never delivered cannot be recovered.

The `FFMPEG` downloader is the default downloader and is the most stable and reliable. It is written in C and is capable of downloading FLV and HLS streams. It provides excellent compatibility for various streaming scenarios, including support for non-standard HEVC in FLV/RTMP containers, and generally has low CPU and memory usage. However, it does not support multithreading for HLS downloads.

> [!NOTE]
> The FFMPEG version provided in our Docker images is a specialized build from [yt-dlp/FFmpeg-Builds](https://github.com/yt-dlp/FFmpeg-Builds/). This build is optimized for streaming and includes (or has upstreamed) critical patches for smooth integration with `yt-dlp`, such as fixing AAC HLS truncation, supporting long paths on Windows, and decoding non-standard HEVC in FLV containers.

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
