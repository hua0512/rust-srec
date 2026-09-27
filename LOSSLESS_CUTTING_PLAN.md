# Lossless cutting for active recordings

Status: implemented. The implementation notes below describe the delivered behavior; the following design sections retain the original planning rationale.

## Implementation notes

- The streamer-card action, authenticated `POST /api/downloads/{download_id}/split` endpoint, OpenAPI documentation, WebSocket state/snapshots, request coalescing, expiration, and localized English/Chinese feedback are implemented.
- Mesio FLV uses the existing ordered pipeline and header cache. Manual cuts require initialized media and a video keyframe or a supported audio packet; they never use the automatic limiter's non-keyframe fallback.
- Mesio HLS validates the first TS media packets or fMP4 sample flags and repeats the current initialization data when splitting. Processing-disabled modes do not advertise this action.
- FFmpeg and Streamlink share [the chunk recorder](rust-srec/src/downloader/engine/chunked.rs), gated by `enable_lossless_cutting` (default `false`, including existing configurations). Their frontend engine settings expose **Enable lossless cutting** with an **Experimental** badge and an explanation of temporary chunks, delayed final files, and additional disk usage. Template overrides preserve inherited, enabled, and disabled settings. Changes apply to new recordings. The chunk recorder keeps acquisition running, preserves continuous chunk timestamps, and finalizes logical recording files asynchronously using stream copy.
- The external-engine path supports MP4, MKV, FLV, TS, and MOV output. When the opt-in is disabled, or custom FFmpeg output arguments or extra Streamlink arguments are present, the existing direct recording path is retained and manual cutting is disabled. Enabled, supported recording modes use temporary disk space even before a cut is requested; final files become available after a cut, an automatic limit, or recording termination.
- Completed files retain the same download/session identity. Final filenames follow the configured template, including `%i`, and receive a numeric suffix such as `-001` only on a collision. Atomic file reservation prevents concurrent recordings from choosing the same name. The attempt UUID and unfinished remux files stay in the staging directory. The manager handles both native close-before-open ordering and asynchronous finalization after the next logical file starts.
- Finalization jobs are bounded. Source chunks and recovery manifests remain on failure. Crash recovery is manual through the retained `group-*.ffconcat` and `group-*.json` files; automatic startup recovery is not implemented.
- Existing split-reason columns store the `manual` reason and request identity. No database migration or dependency change is needed.

## Local validation

Validation was performed on Windows with default Rust features, FFmpeg `N-122015-g6a14a93af5-20251207`, and Streamlink `8.0.0`.

| Check | Result |
| --- | --- |
| Media/pipeline library tests | 208 passed |
| Backend downloader, downloads API, and OpenAPI tests | 287 passed |
| Frontend suite | 963 passed |
| Real-media integration tests | 3 passed; explicitly enabled despite their default ignored status |
| Workspace Clippy, all targets excluding the desktop Rust wrapper | Passed with warnings denied |
| Rust formatting and final diff whitespace checks | Passed |
| Frontend formatting, lint, and type checking | Passed |
| Web, desktop frontend, and documentation builds | Passed |

The real-media tests generate H.264/AAC fixtures locally. They verify unchanged decoded video frames and encoded audio packets across native FLV, TS, and fMP4 cuts; FFmpeg MP4, MKV, FLV, TS, and MOV output; stop-during-cut and automatic-rotation cases; and an actual Streamlink process consuming a loopback HLS fixture without re-fetching media segments. Tests also cover request/stop races, authorization, stale download IDs, malformed media boundaries, and retained recovery files after finalizer failure.

Other codec combinations, mid-stream codec changes in external engines, Linux execution, and the complete CI matrix were not exercised by these local media fixtures. CI remains the integration gate. See [the engine guide](rust-srec/docs/en/concepts/engines.md#lossless-cutting) for availability and recovery behavior.

## Intended behavior

Add a **Lossless cutting** action to the streamer card's actions menu. The action finishes the current recording file and continues recording into another file without re-encoding or reconnecting because of the cut.

The user confirmed that this means splitting an active recording, rather than trimming an existing recording.

The cut waits for a safe media boundary, so it may happen shortly after the click. The action must preserve delivered media across the two files, keep the same download and session, and allow each output file to play independently. It cannot recover media that the source never delivered.

Recommended rollout: implement Mesio first, then add a shared recording-output mechanism for FFmpeg and Streamlink.

## Engine feasibility

| Engine / mode | Existing implementation | Proposed approach | Scope |
| --- | --- | --- | --- |
| Mesio FLV | Already rotates files, caches headers, and detects keyframes | Request rotation at the next suitable video keyframe; use an audio packet boundary for audio-only streams | Moderate |
| Mesio HLS | Already rotates between delivered segments and reinserts fMP4 initialization data | Request rotation before the next independently decodable segment | Moderate, with additional boundary validation |
| FFmpeg | Uses stream copy and startup-configured segmentation | Continuously produce internal chunks and assemble them into user-visible recording files | Substantial; prototype required |
| Streamlink | Pipes its output into FFmpeg | Keep Streamlink running and reuse the proposed FFmpeg output mechanism | Substantial, mostly shared |
| Mesio with consistency processing disabled | Bypasses the split operators needed by the proposed implementation | Initially report cutting as unsupported; later add a minimal parser/splitter with the same guarantees | Deferred |

## Mesio FLV

Relevant files:

- [FLV limit operator](crates/flv-fix/src/operators/limit.rs)
- [FLV pipeline](crates/flv-fix/src/pipeline.rs)
- [FLV writer](crates/flv-fix/src/writer_task.rs)
- [Backend FLV downloader](rust-srec/src/downloader/engine/mesio/flv_downloader.rs)

Extend the existing limit operator to accept a pending manual cut. At a safe boundary:

1. Emit a split marker with a manual reason.
2. Reinsert cached FLV and codec headers.
3. Send the boundary frame into the new file.
4. Continue through the existing timestamp handling and writer.

Keep boundary selection in the ordered media-processing path so buffered data cannot bypass the split. Audio-only streams can split at an audio packet boundary.

The automatic limiter currently has a fallback that can force a split on a non-keyframe after substantially exceeding a configured limit. A manual lossless cut must not use that fallback. If a safe boundary never arrives, expire the request while recording continues.

Manual cuts must work when automatic duration and size limits are both disabled. Coordinate manual and automatic rotation so a coincident boundary does not produce two cuts or an empty file.

## Mesio HLS

Relevant files:

- [HLS segment limiter](crates/hls-fix/src/operators/segment_limiter.rs)
- [HLS pipeline](crates/hls-fix/src/pipeline.rs)
- [HLS media helpers](crates/hls/src/segment.rs)
- [HLS writer](crates/hls-fix/src/writer_task.rs)
- [Backend HLS downloader](rust-srec/src/downloader/engine/mesio/hls_downloader.rs)

Extend the segment limiter so manual and automatic rotation share initialization-data handling and counter resets. Preserve complete media segments, and ensure the new fMP4 file receives the latest initialization segment before dependent media. Keep manual-cut support available independently of the automatic-limiter setting when the required processing is enabled.

Boundary validation needs improvement before enabling this action:

- The current TS helper checks whether a segment contains a random-access indicator. This alone does not establish that the segment starts with independently decodable video and the required container information.
- The current fMP4 helper treats a leading `moof` box as evidence of a keyframe. A fragment header alone does not establish sample independence.
- Propagate playlist independence information where available and validate the starting media samples otherwise. Preserve the required TS program tables and codec information.
- Handle audio-only streams separately from video keyframe requirements.

HLS explicitly distinguishes independent segments through `EXT-X-INDEPENDENT-SEGMENTS`. See [RFC 8216, section 4.3.5.1](https://www.rfc-editor.org/rfc/rfc8216.html#section-4.3.5.1).

If independence cannot be established, do not report a successful safe cut. Wait for a supported boundary or expire the request without stopping recording.

## FFmpeg and Streamlink

Relevant files:

- [FFmpeg engine](rust-srec/src/downloader/engine/ffmpeg.rs)
- [Streamlink engine](rust-srec/src/downloader/engine/streamlink.rs)
- [Streamlink stop control](rust-srec/src/downloader/engine/streamlink/control.rs)
- [FFmpeg event tracking](rust-srec/src/downloader/engine/utils/ffmpeg_tracker.rs)

### Current limitation

The FFmpeg engine supplies `-c copy` and optional startup-configured `-segment_time`. Streamlink supplies media through stdout to a separate FFmpeg remuxing process with a similar output arrangement.

The investigation checked local FFmpeg segment-muxer help and upstream source. The stock CLI provides no supported runtime "split now" command for the segment muxer. Interactive commands target filters, while `q` quits. Updating the shared download configuration does not reconfigure those running processes. The repository's Streamlink control channel is for stopping acquisition, not output rotation.

References:

- [FFmpeg segment muxer documentation](https://ffmpeg.org/ffmpeg-formats.html#segment_002c-stream_005fsegment_002c-ssegment)
- [FFmpeg segment muxer source](https://ffmpeg.org/doxygen/trunk/segment_8c_source.html)
- [FFmpeg CLI command handling](https://ffmpeg.org/doxygen/trunk/ffmpeg_8c_source.html)

### Proposed shared output mechanism

This design requires a prototype before committing to production support:

1. Keep the recording process running and configure FFmpeg at startup to continuously produce short, uniquely numbered internal chunks using stream copy.
2. Group those chunks into logical recording files. Internal chunks remain invisible to recording history and post-processing.
3. On a manual cut request, close the current group at the next safe chunk boundary after the request. Subsequent chunks belong to the next group.
4. Finalize the closed group through concat/remux with stream copy.
5. Publish the normal segment-completed event only after the final output file is ready.

Streamlink continues feeding the same persistent FFmpeg process throughout. Share grouping, finalization, recovery, and cut-request handling between the two external engines.

Prototype and resolve these constraints:

- Container and codec compatibility, including independently decodable chunk starts and codec changes.
- Audio/video timestamp continuity across chunks and final files.
- Added disk I/O, temporary space, finalization latency, and bounded finalization backlogs.
- Recovery of unfinished groups after a crash; retain source chunks until finalization succeeds.
- Accurate recording progress and logical segment timestamps despite delayed finalization.
- Automatic duration/size limits and manual cuts operating on logical files rather than exposing every internal chunk.
- Unique filenames, safe publication of finalized outputs, and Windows/Linux behavior.
- Capability validation for custom FFmpeg arguments, especially options that change codecs or output layout.
- Continued acquisition while previous groups finalize, with explicit handling of storage exhaustion or finalization failure.

For this design, the UI must distinguish a cut request from file finalization. Completing the current logical recording file may take longer than reaching the cut boundary.

### Alternative

A controllable muxer built with libavformat could rotate output directly while keeping acquisition running. This offers more direct control but adds native dependencies and packaging work. Prototype the chunk approach first because it retains compatibility with ordinary FFmpeg installations.

Neither stopping/restarting FFmpeg nor switching Streamlink's byte pipe at an arbitrary offset satisfies the intended continuity and playable-file guarantees.

## Shared control interface

Relevant files:

- [Download handle and engine types](rust-srec/src/downloader/engine/traits.rs)
- [Download manager](rust-srec/src/downloader/manager.rs)
- [Download attempt event handling](rust-srec/src/downloader/manager/attempt.rs)
- [Download manager events](rust-srec/src/downloader/manager/events.rs)

Add a per-download cut controller alongside `DownloadHandle`, with a small interface for capability, requesting a cut, and observing request status. Keep cut requests separate from cancellation.

Required behavior:

- Determine capability from the actual running engine, protocol, and processing configuration.
- Accept a bounded pending request with an identifier and deadline.
- Coalesce repeated clicks while a cut is pending.
- Distinguish accepted, waiting for a safe boundary, finalizing where applicable, completed, expired, cancelled, and failed outcomes.
- Resolve races with automatic rotation without creating duplicate cuts or empty files. A coincident automatic rotation may satisfy the request only when it meets the manual cut's safety requirements.
- Remove expired requests so they cannot cause an unexpected later cut.
- Cancel pending requests when the download stops or ends; do not carry them into a replacement attempt.
- Report success only after the old file is finalized and recording has continued into the next logical file. EOF without a continuation is not a successful manual split.
- Keep request state observable after a WebSocket reconnect.

Do not stop recording merely because a safe cut boundary could not be found. Actual writer failures follow the existing recording failure and output-root handling.

## Backend endpoint

Proposed mutation:

```http
POST /api/downloads/{download_id}/split
```

Target the displayed download ID so a stale click cannot affect a replacement recording for the same streamer.

- Use the existing authentication and mutation authorization conventions.
- Return `202 Accepted` with a request ID and current request state.
- Reject unknown, unsupported, or stopping downloads with explicit machine-readable errors.
- Treat repeated requests during a pending cut consistently by returning the existing pending request.
- Expose completion and failure through observable request state, rather than treating HTTP acceptance as completed cutting.

The current [downloads router](rust-srec/src/api/routes/downloads.rs) serves a WebSocket endpoint that performs its own authentication. Ensure the new mutation receives the appropriate authentication and write-access checks when it is wired into the router.

## Segment lifecycle and persistence

Relevant files:

- [Canonical split reasons](crates/media-types/src/split_reason.rs)
- [Mesio writer callbacks and reason mapping](rust-srec/src/downloader/engine/mesio/helpers.rs)
- [Session segment model](rust-srec/src/database/models/session.rs)
- [Pipeline event handling](rust-srec/src/pipeline/manager/events.rs)
- [Download progress protobuf](rust-srec/proto/download_progress.proto)
- [Frontend split-reason display](rust-srec/frontend/src/lib/split-reason.ts)

Add `SplitReason::Manual` and map it to the stable code `manual`. Carry request correlation through the control/event path so a completed automatic segment cannot accidentally confirm an unrelated pending cut.

Reuse existing segment-started and segment-completed processing. Preserve download/session identity, session-scoped segment numbering, and danmaku/pipeline coordination. Keep media boundary timestamps accurate if external-engine file finalization is delayed.

Existing session-segment fields already store split-reason codes and details. No database migration appears necessary for the manual reason itself; reassess if the final design adds durable request or chunk-recovery state.

Add capability and cut-request state to WebSocket metadata/snapshots. The current WebSocket segment-completed mapping leaves `split_reason` empty; wire the reason through as part of this change. Regenerate protobuf bindings rather than editing generated files.

## Frontend action

Relevant files:

- [Streamer card](rust-srec/frontend/src/components/streamers/streamer-card.tsx)
- [Actions menu](rust-srec/frontend/src/components/streamers/card/stream-actions-menu.tsx)
- [Download state store](rust-srec/frontend/src/store/downloads.ts)
- [Download progress hook](rust-srec/frontend/src/hooks/use-download-progress.ts)
- [Existing server-function conventions](rust-srec/frontend/src/server/functions/streamers.ts)

Add **Lossless cutting** with a scissors icon to the actions menu.

- Enable it for a supported active recording; derive availability from backend capability rather than only the streamer being live.
- Explain unavailability for unsupported recording modes.
- Show "Waiting for a safe cut..." while waiting and a finalization state where applicable.
- Prevent duplicate submissions while pending.
- Confirm success only after the backend reports completion; display expiration/failure without implying recording has stopped.
- Preserve request state across WebSocket reconnects and handle the active download ending or being replaced.
- Include localized text, keyboard accessibility, and the existing card-selection behavior.
- Use the project's request abstraction so both web and desktop deployments work.
- Keep high-frequency progress subscriptions out of the card action's rendering path where possible.

## Implementation sequence

1. Add the shared controller, capability model, request lifecycle, and manual split reason.
2. Implement and validate Mesio FLV cutting.
3. Strengthen HLS boundary detection and implement Mesio HLS cutting.
4. Wire the endpoint, snapshots/events, frontend action, and localization. Enable only validated modes.
5. Prototype continuous internal chunks and stream-copy finalization for FFmpeg.
6. Validate the external-engine design and integrate Streamlink through the shared output mechanism.
7. Update engine feature documentation in both locales and complete consumer/platform validation.

## Validation plan

Use deterministic local media fixtures and bounded waits. Validate observable recording behavior rather than only internal command construction.

### Media and lifecycle tests

- Both files decode independently with the necessary headers/initialization data.
- No cut-induced missing or duplicated media samples across the boundary; compare packet/sample continuity and payloads while accounting for container/timestamp changes.
- The recording retains its download and session identity.
- Video, audio-only, long-GOP, missing-keyframe, and unsupported-codec behavior.
- FLV header reinjection and timestamp handling.
- HLS TS/fMP4 independence detection, initialization changes, discontinuities, and required program tables.
- Repeated clicks, concurrent requests, and simultaneous automatic/manual rotation.
- EOF, stop, cancellation, deadline expiration, and replacement-download races.
- No empty files or filename collisions, including multiple cuts within one second.
- Output failures and correct segment persistence, danmaku coordination, and pipeline triggers.

### External-engine tests

- No cut-induced recording-process restart or source reconnection.
- Correct grouping and lossless remux for supported container/codec combinations.
- Continued acquisition while a previous group finalizes.
- Chunk retention, crash recovery, finalization failures, and bounded backlog behavior.
- Accurate logical segment timestamps and progress without publishing internal chunks.
- Relevant Windows and Linux process/filesystem behavior.

### API and frontend tests

- Authentication, write-access enforcement, capability rejection, and stale download IDs.
- HTTP acceptance versus eventual completion, expiration, or failure.
- Snapshot/WebSocket restoration of capability and pending state.
- Action availability, duplicate-submit prevention, status feedback, and card-selection behavior.
- Web and desktop request-path compatibility.

### Repository checks

Follow the applicable `AGENTS.md` requirements: Rust formatting, affected-package Clippy, and focused tests; expand to affected consumers of shared media types and pipeline interfaces. Run frontend formatting, lint, relevant tests, and type checking, with web/desktop builds for the affected integration. Regenerate protobuf outputs and build the documentation site when its feature documentation changes.

## Investigation evidence and limitations

The plan is based on inspection of the repository's frontend actions, download controls, all three engine implementations, media split operators, writers, segment lifecycle, and progress messages. Local `ffmpeg -hide_banner -h muxer=segment` and upstream FFmpeg/HLS sources were checked.

Planning initially left the external-engine design unproven. The implementation and local media tests described above now exercise that path, along with stricter native HLS boundary validation and the FLV manual-cut guard. The original design constraints remain useful for extending codec and platform coverage.
