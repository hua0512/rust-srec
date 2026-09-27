# Lossless cutting for active recordings

Status: implemented. This document describes the current design. The [engine guide](rust-srec/docs/en/concepts/engines.md#lossless-cutting) explains configuration and recovery.

## Recording behavior

The streamer card's **Lossless cutting** action finishes the current recording file and continues in a new file within the same download and session. It does not reconnect to the source or re-encode media.

A request waits for a safe media boundary. Repeated requests return the pending request's ID. If no suitable boundary arrives within 30 seconds, the request expires and recording continues. Once a boundary has been selected, file finalization can take longer. Success requires both the completed old file and the start of the next file.

Stopping a recording rejects further requests and cancels pending cuts. A replacement recording has a separate controller. Writer and finalizer failures follow the existing recording failure handling.

## Engine support

| Engine | Implementation and requirements |
| --- | --- |
| Mesio FLV | Uses the ordered limit operator and cached FLV/codec headers. Requires consistency processing. Video cuts wait for a keyframe; supported audio-only cuts use packet boundaries. Manual cuts never use the automatic limiter's non-keyframe fallback. |
| Mesio HLS | Uses the ordered segment limiter and validates the first TS media packets or fMP4 sample flags. Requires consistency processing and an independently decodable segment. The next fMP4 file receives the current initialization data. |
| FFmpeg | Requires `enable_lossless_cutting: true`, MP4/MKV/FLV/TS/MOV output, and no custom output arguments. Uses the shared chunk recorder. |
| Streamlink | Requires the same opt-in and output formats, with no extra Streamlink arguments. Streamlink continues feeding FFmpeg while the shared chunk recorder finalizes files. |

The FFmpeg and Streamlink setting defaults to `false`, including when it is absent from an existing configuration. Their frontend forms display **Enable lossless cutting**, an **Experimental** badge, and an explanation of temporary chunks, delayed final files, and additional disk usage. Template overrides support inheritance, explicit enablement, and explicit disablement. Changes apply to new recordings.

When the option is disabled or the output mode is unsupported, FFmpeg and Streamlink retain their ordinary recording behavior. Mesio support depends on its consistency-processing settings instead of this option.

## Chunk recording and files

The [shared chunk recorder](rust-srec/src/downloader/engine/chunked.rs) keeps acquisition running and groups private Matroska chunks into recording files. It preserves continuous chunk timestamps and combines each group with FFmpeg stream copy after a manual cut, an automatic limit, or the end of recording. Files become available after finalization, even if no manual cut was requested.

Chunks, concat manifests, recovery metadata, and unfinished remux files remain in `.srec-chunks-{download_id}` inside the output directory. Final filenames follow the configured template, including `%i`. Atomic file reservation adds a suffix such as `-001` only when a filename is occupied, preventing concurrent recordings from overwriting each other.

Finalization runs while acquisition continues, with a bounded queue. This requires extra disk space and I/O. Source chunks and recovery manifests remain if finalization fails. Crash recovery is manual: `group-*.json` identifies the intended output and `group-*.ffconcat` lists the closed chunks. An unfinished final chunk may need separate inspection. Staging files are removed only after their final recording file is published.

## Requests and status

The [manual split controller](crates/pipeline-common/src/manual_split.rs) serializes request acceptance, boundary selection, expiration, and shutdown for one recording attempt. Its status and revision are included in WebSocket updates and reconnect snapshots.

`POST /api/downloads/{download_id}/split` requires full access and returns `202 Accepted` with a request ID and state. It targets the current download ID so a stale click cannot affect a replacement recording. Unknown downloads return `404`; unsupported or stopping downloads return `409`. HTTP acceptance does not indicate completion.

The [download manager](rust-srec/src/downloader/manager/attempt.rs) handles both event orders: native writers close the old file before starting the next, while external engines may finalize the old file after the next logical file starts. The existing segment records store the `manual` reason and request ID. No database migration is required.

The frontend disables the action when the recording is unsupported, disconnected, or already processing a cut. It distinguishes waiting, finalizing, completion, expiration, cancellation, and failure. English and Simplified Chinese translations cover the action and engine settings.

## Validation

Tests cover request coalescing, expiration, shutdown, stale IDs, authorization, event ordering, configuration inheritance, filename collisions, concurrent reservations, and retained recovery files after finalizer failure.

Generated H.264/AAC fixtures verify media preservation across native FLV/TS/fMP4 cuts and FFmpeg MP4/MKV/FLV/TS/MOV output. Streamlink tests use a local HLS server to check that cuts do not restart acquisition. External engines are exercised with the opt-in disabled and enabled.

Completed recordings are compared by decoded video-frame hashes and encoded audio-packet hashes. Stop-during-cut tests compare encoded video packets with the received prefix of the source: B-frame reordering can make decoded output differ from a display-order prefix when acquisition stops mid-GOP.

Local validation uses Windows with default Rust features. Other codec combinations, mid-stream codec changes in external engines, and Linux execution were not tested locally. Automatic crash recovery is not implemented.
