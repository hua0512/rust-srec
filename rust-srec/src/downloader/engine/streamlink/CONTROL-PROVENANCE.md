# Streamlink recording control provenance

The companion is embedded by `control.rs` and loaded by the configured CLI's
`--plugin-dir` loader. It exports a plugin with no URL matcher and retains the
CLI's platform selection, quality selection, configuration files and arguments.
Loader probing is bounded and coalesced; each recording gets a fresh authenticated
connection and checks the reader graph again. The temporary source directory is
owned until the contained child exits. This does not change runtime images.

The initial profile is Streamlink 8.5.0, audited from the upstream wheel
`streamlink-8.5.0-py3-none-any.whl`, SHA-256
`14595986ce0ed098fba7944e6d246b2d6b49985f7a1deb8f5d2cee22c17df1e4`.
[Upstream sources](https://github.com/streamlink/streamlink/tree/8.5.0) are checked
at runtime using the source digests in `SOURCES`, `HTTP_SOURCES` and
`WEBSOCKET_SOURCES`. Missing source, modified lifecycle methods, opaque transports
and unknown reader graphs cannot establish the cooperative guarantee.

Relevant upstream seams:

- `streamlink_cli/main.py`: plugin loading, `open_stream` and its 8192-byte prebuffer.
- `streamlink_cli/streamrunner.py`: its normal read-to-EOF output loop remains in use.
- `streamlink/stream/segmented/segmented.py`: worker closure and the FIFO sentinel,
  preserving admitted segment and initialization-map jobs before buffer closure.
- `streamlink/stream/wrappers.py`: the filler loop is adapted to retain its current
  read/write and drain HTTP buffering before producer cleanup.
- `streamlink/stream/filtered.py`: the read loop is adapted to wake on stop and
  continue waiting for already-admitted data using the remaining stop budget.
- `streamlink/stream/ffmpegmux.py`: nested muxers remain alive to EOF; pipe writes
  are checked and natural process exit is required before destructive cleanup.
- `streamlink/utils/named_pipe.py`: Windows writes are completed, and accepted
  bytes are flushed before disconnect. The Windows API's
  [disconnect contract](https://learn.microsoft.com/en-us/windows/win32/api/namedpipeapi/nf-namedpipeapi-disconnectnamedpipe)
  otherwise permits unread bytes to be discarded.
- `streamlink/utils/processoutput.py`: the internal FFmpeg validation remains
  enabled and receives the same hidden-window creation flag as actual muxers.

HTTP uses the audited requests 2.34.2 / urllib3 2.7.0 response path on CPython 3.11
and 3.14. Acquisition is gated below `http.client`'s existing `BufferedReader`, at
the instance `SocketIO.readinto` boundary. Decrypted TLS pending data, HTTP
prefetch and gzip/deflate decoder tails continue draining. Stop-induced transport
EOF is distinguished from an upstream error by the acquisition gate. Optional
Brotli/Zstandard decoders and opaque socket adapters retain ordinary recording
without a cooperative claim. No urllib3/requests source is copied into the helper.

The websocket profile additionally checks websocket-client 1.9.2 sources. Its
`recv_data_frame` through `on_data` delivery is one admitted operation; stop cannot
close an in-flight receive or discard an already-received frame. Existing frame
and TLS buffers are accounted for before admitting no further messages.

`FileStream` paths use an unbuffered file gate. BytesIO input is already acquired
and is drained completely. Arbitrary buffered `fileobj` instances and custom
iterators retain their existing reads but cannot claim a proven acquisition stop.

The stop deadline bounds acquisition, child-process containment, stdout forwarding
and remux finalization. A later shutdown can tighten an already-stopping handle
through a monotonic watch notification. Required final-event delivery remains
owned until its consumer accepts it; direct engine callers must drain that
channel, and the worker's force cap handles a stalled consumer.

## License of adapted Streamlink code

Copyright (c) 2011-2016, Christopher Rosell
Copyright (c) 2016-2026, Streamlink Team
All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

1. Redistributions of source code must retain the above copyright notice, this
   list of conditions and the following disclaimer.
2. Redistributions in binary form must reproduce the above copyright notice,
   this list of conditions and the following disclaimer in the documentation
   and/or other materials provided with the distribution.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND
ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT OWNER OR CONTRIBUTORS BE LIABLE FOR
ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES
(INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES;
LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND
ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
