# Offline Streamlink control tests

Install `requirements.txt` into a task-owned Python environment. These are test
dependencies, not changes to the runtime image or the configured recording CLI.
CPython 3.11 and 3.14 are the audited HTTP interpreter profiles.

Run the pure acquisition tests from the repository root:

```sh
python -m unittest discover -s rust-srec/tests/streamlink_control -p test_companion.py
python -m unittest discover -s rust-srec/tests/streamlink_control -p test_process_tree.py
```

They use actual Streamlink/requests/urllib3 classes, local sockets and ephemeral
TLS certificates. They do not launch native subprocesses or contact live services.
The suite covers source-profile refusal, filtering, full rings, HLS maps/AES and
DASH response writing, HTTP/TLS/decoder buffering, websocket receive/delivery
ownership, startup, file objects and checked Windows pipe operations.
The launcher unit tests replace and guard real process/signal APIs while checking
retained process identity, admission failures, output limits and bounded cleanup.

The separate native suite requires `ffmpeg`, `ffprobe` and `rustc` on PATH as well
as the pinned Streamlink environment. Set `SREC_STREAMLINK_NATIVE=1` in that
command's environment, then run:

```sh
python -m unittest discover -s rust-srec/tests/streamlink_control -p test_native.py
```

Native fixtures generate finite local media. The stop and natural-EOF comparison
admit exactly four jobs per track and hold the final accepted fetch until producer
stop, then compare packet data hashes and timestamps through two nested muxers and
a sparse final subtitle. Independent packet-count assertions catch common
truncation in both comparison outputs. Other fixtures exercise actual CLI help
plugin loading and demonstrate same-version opaque executables that reject or
ignore control flags. CLI launches disable user configuration and automatic plugin
sideloading. A native shim observes both FFmpeg validation and muxer console handles;
its Windows negative control creates a hidden console to verify the observer.
No native fixture uses a shell or an execute processor.

The native launcher enrolls Windows children in a kill-on-close Job before they
execute, and preserves the POSIX leader identity until group cleanup. Exited
leaders still retain descendant ownership. Captured streams have a 4 MiB limit;
command timeouts retain a separate bounded cleanup allowance (five seconds by
default). Native regression fixtures cover an exited leader's inherited pipe,
timeout and output overflow. Caller-provided media/log files retain their normal
file ownership and do not use the capture limit.

The Rust selections are `downloader::engine::streamlink` and the deadline-watch
test under `downloader::engine::traits`. They cover authenticated control framing,
probe coalescing/invalidation, child-exit ordering, late/tightened deadlines,
console-independent tail draining, early acknowledgements, output-error precedence,
and actual backend recording after rejected or ignored plugin capability probes.
These native suites must be run by the repository's serialized
validation owner; skipped optional native tests are not evidence of a passed
native integration.

See [source provenance](../../src/downloader/engine/streamlink/CONTROL-PROVENANCE.md)
for the audited upstream versions, boundaries and adapted-code license.
