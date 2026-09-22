# Recording engine internals

For contributors changing subprocess supervision or recording events. For engine selection and configuration, see [Recording engines](../concepts/engines.md).

## Stopping Streamlink Recordings {#stopping-streamlink-recordings}

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

## Recording Filenames and Telemetry {#recording-filenames-and-telemetry}

Recording templates expand their configured date tokens, while percent signs in
streamer names and titles remain literal. For example, a title `Top 5%d` stays
`Top 5%d`; it does not substitute the day of the month. Startup output-root probes
use the same literal metadata rules.

FFmpeg recording and Streamlink remuxing require info-level logs and statistics
to track segments and progress. The backend supplies final `-loglevel info
-stats` options; custom quiet log settings and `-nostats` do not disable this
telemetry.
