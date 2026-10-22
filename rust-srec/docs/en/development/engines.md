# Recording engine internals

For contributors changing subprocess supervision or recording events. For engine selection and configuration, see [Recording engines](../concepts/engines.md).

## Stopping Streamlink Recordings {#stopping-streamlink-recordings}

A stop sends SIGTERM to the Streamlink process on Unix. Hidden Windows children
have no console-signal contract, so they receive no request. The backend then
waits for Streamlink to exit for at most three seconds and force-terminates its
contained process tree when it does not. Stdout forwarding and stderr processing
remain alive until the pipe reaches EOF; FFmpeg's stdin then closes so it can
finalize the output.

A forced Streamlink exit is an ordinary stop rather than a failure: FFmpeg
finalizes every byte forwarded before the pipe closed, and data still buffered
inside Streamlink is lost. Cleanup errors, an FFmpeg failure and a stdout pipe
failure still fail the attempt.

The attempt's remaining stop deadline bounds these process, forwarding and remux
phases, with time reserved for FFmpeg finalization. A later shutdown can tighten
an already-stopping attempt. Required final-event delivery remains owned until its
consumer accepts it: direct engine integrations must drain the event channel,
and a stalled consumer is bounded by the worker's overall force cap. Deadline
expiry still forces contained process-tree termination and can truncate the tail.

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
