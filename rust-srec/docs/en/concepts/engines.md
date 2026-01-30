# Engines

Downloaders are the core of the application. They are responsible for downloading the video stream from the source. The application supports three downloaders: `Mesio`, `FFMPEG`, and `Streamlink`. Each downloader has its own features and limitations.

> [!TIP]
> For **Mesio** users, it is **highly recommended** to enable both **FLV Consistency Fix** and **HLS Consistency Fix**. These features ensure the recorded files remain playable and consistent even when the source stream encounters issues like timestamp jumps or network interruptions.

The `FFMPEG` downloader is the default downloader and is the most stable and reliable. It is written in C and is capable of downloading FLV and HLS streams. It provides excellent compatibility for various streaming scenarios, including support for non-standard HEVC in FLV/RTMP containers. It is the most efficient downloader in terms of CPU and memory usage. However, it does not support multithreading for HLS downloads.

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

## 2. FLV Consistency Fix Feature List

|                          Feature                           | Engine Action   |
| :--------------------------------------------------------: | --------------- |
|                      Timestamp Jumps                       | Fix using delta |
|    Video Header Changes (Resolution, Other Parameters)     | Split file      |
|                    Audio Header Changes                    | Split file      |
| AMF Metadata Injection (lastheadertimestamp, keyframes...) | Inject          |
|                Duplicate TAG (experimental)                | Ignore          |

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

When using the Mesio engine for HLS streams, the **HLS Consistency Fix** feature can automatically detect and resolve common HLS delivery issues, such as timestamp discontinuities or missing segments, before the data is written to the final file. This ensures a smoother playback experience for the recorded content.
