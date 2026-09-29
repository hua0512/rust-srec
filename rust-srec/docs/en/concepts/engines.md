# Recording engines {#engines}

Use Mesio for a first recording. Select FFmpeg when you need its container or codec compatibility, or Streamlink for its supported download behavior. FFmpeg and Streamlink must be installed on the backend host; the Docker image includes the supported tools.

Streamlink can also be an **extractor**, which resolves the stream URL before downloading. Extractor selection and download-engine selection are independent. See [engine and extractor settings](../reference/configuration-overrides.md#engine-and-extractor-selection).

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

For Mesio, enable FLV and HLS Consistency Fix when you need their repair and segmentation features. They cannot recover media the source never delivered.

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

**Loop Protection** (`flv_fix.duplicate_tag_filtering`) is off by default. When enabled, it removes retained media tags with identical bytes, type, stream ID, and timestamp, including loops that jump back to earlier timestamps. With Offset Consistency Check off, a timestamp jump alone does not clear exact-match history: legitimate repeated content with the same timestamps may also be removed. Source headers, configuration/control packets, and explicit discontinuities reset that history.

**Offset Consistency Check** (`flv_fix.duplicate_tag_filter_config.enable_replay_offset_matching`) is separately off by default. Enable it to detect loops whose timestamps have shifted. It requires three distinct payloads in order at one consistent timestamp offset; ambiguous or incomplete matches pass through. Matching uses stream timestamps, so pauses between GOP deliveries do not expire it. This remains a heuristic; leave Loop Protection off to preserve all incoming media tags.

When enabled, filtering copies retained media payloads, up to 8,192 tags and **16 MiB per recording stream** by default, plus indexing overhead. Offset matching may retain another **1 MiB** of pending payloads. The advanced configuration field `flv_fix.duplicate_tag_filter_config.window_capacity_bytes` controls the history byte budget (default `16777216`; `0` disables retention); it has no dedicated form field. Payloads larger than the budget pass through. No duplicate-filter history is allocated while Loop Protection is off.

## 3. Raw Data Mode

In Raw Data Mode, the **Mesio** engine writes stream data directly to disk as it arrives from the network, without parsing or processing media packets (headers, frames, metadata).

### Key Characteristics:
- **Reduced processing**: Skipping packet parsing and re-muxing reduces CPU and memory use.
- **Use cases**: High-bandwidth streams or resource-constrained environments (such as a low-end NAS or VPS). Use this mode only when the network is stable and the CDN or stream source does not change media headers or other stream structure.

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

## Lossless cutting {#lossless-cutting}

During a supported recording, open the streamer card's **Actions** menu and select **Split file now**. The action appears only on recordings that support lossless cutting. While a cut is in progress, the card shows **Splitting…** and then **Finalizing…**. The recorder finishes the current file and continues in a new file within the same recording session. The cut does not re-encode media or reconnect to the source.

The action waits for a safe media boundary. Repeated clicks share the pending request. If no suitable boundary arrives within 30 seconds, the request expires and recording continues. The notification explains why: no new media arrived, no keyframe arrived (FLV), the stream's HLS segments are not marked as independently playable, or earlier files were still being finalized (FFmpeg and Streamlink). File finalization can take longer; success is shown after the old file is ready and the next file has started.

| Engine | Availability |
| --- | --- |
| Mesio FLV | Requires FLV consistency processing and the headers needed to start the next file. Video cuts wait for a keyframe. |
| Mesio HLS | Requires HLS consistency processing and an independently decodable TS or fMP4 segment. |
| FFmpeg | Requires experimental **Enable lossless cutting**, MP4, MKV, FLV, TS, or MOV output, and no custom output arguments. |
| Streamlink | Requires experimental **Enable lossless cutting**, the same output formats, and no extra Streamlink arguments. |

In the download engine configuration, turn on **Enable lossless cutting** for the FFmpeg or Streamlink engine you use. This setting is marked **Experimental** and is off by default, including for existing configurations. The JSON field is `enable_lossless_cutting` (default `false`). Changes apply to new recordings. Template engine overrides can enable or disable it explicitly, or select **Use engine setting** to inherit the engine configuration. Mesio does not require this option.

When enabled for a supported FFmpeg or Streamlink mode, recording uses temporary chunks and assembles each final file after a manual cut, an automatic limit, or the end of recording. Final files become available only after assembly finishes, even when no manual cut is requested. This uses additional disk space and I/O during finalization. Final filenames follow your configured template, including `%i` when present. A numeric suffix such as `-001` is added only if that filename already exists. The recording attempt ID and unfinished remux files stay inside the `.srec-chunks-*` staging directory. When the option is off, or the output format or custom arguments are unsupported, the engine retains its existing recording behavior and the action does not appear. The engine settings warn when custom arguments prevent lossless cutting.

Stopping a recording ends acquisition within the engine's graceful stop timeout, then waits for the remaining files to be assembled, however long that takes. Only an application shutdown that runs out of time interrupts assembly. If FFmpeg or Streamlink exits unexpectedly, the chunks captured before the exit are still assembled; the unfinished last chunk stays in the staging directory.

If finalization fails, is interrupted by shutdown, or the backend stops unexpectedly, preserve the `.srec-chunks-*` directory in the recording output folder. Its `group-*.json` files identify the intended outputs, and `group-*.ffconcat` files describe the closed chunks. A retained manifest can be remuxed to a **new** output file using FFmpeg's concat demuxer with stream copy. Recovery is manual; an unfinished last chunk may require separate inspection. Do not delete the staging files until the recovered output has been checked.

## Streamlink FFmpeg Executable {#streamlink-ffmpeg-executable}

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

The engine editor exposes **FFmpeg Path** and preserves it when other settings
change. Clearing the field or choosing **Use environment default** restores the
environment/default lookup. Enter the path on the backend server without shell
quotes; path text is preserved exactly.

In a template's Streamlink override, clearing the field or choosing **Use engine
setting** removes the override and inherits the engine path. **Use environment
default** instead writes an explicit `null`, clearing the engine path for that
template so `FFMPEG_PATH` or `ffmpeg` is used.

## Executable Checks {#executable-checks}

FFmpeg and Streamlink version checks have a three-second deadline, followed by bounded process cleanup if needed. Engine tests and custom-engine resolution run these checks asynchronously. An executable that cannot be started or does not finish its version check before the deadline is reported unavailable. Synchronous startup initialization waits for the same bounded check.

Missing named engine configurations retain the default-engine fallback. Database access failures, malformed saved settings, and invalid overrides fail resolution instead of silently changing the effective configuration.

## Stopping Streamlink Recordings {#stopping-streamlink-recordings}

Supported Streamlink 8.5.0 readers can stop acquiring new data while buffered data is forwarded to FFmpeg for finalization. This must finish within the recording's stop deadline. Forced stops can truncate the final output.

Old, portable, or custom executables and unsupported readers keep their ordinary recording behavior, but stopping may report `Streamlink cooperative drain incomplete`. See [supported reader profiles and stopping mechanics](../development/engines.md#stopping-streamlink-recordings).

For download-session and media-repair architecture, see [Mesio internals](./mesio.md).

<div id="recording-filenames-and-telemetry" class="legacy-section">

This section is now in [Recording engine internals](../development/engines.md#recording-filenames-and-telemetry).

</div>

<div id="_5-mesio-architecture" class="legacy-section">

This section is now in [Mesio Engine](./mesio.md).

</div>
