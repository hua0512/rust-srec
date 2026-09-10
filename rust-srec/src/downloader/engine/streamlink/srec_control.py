"""Private recording control plugin. See CONTROL-PROVENANCE.md for the audited ABI.

This adds no URL matcher and changes no platform resolution or CLI arguments.
Only an authenticated stop closes acquisition; consumers keep reading to EOF.
"""

# Copyright (c) 2011-2016, Christopher Rosell
# Copyright (c) 2016-2026, Streamlink Team
# All rights reserved.
#
# Redistribution and use in source and binary forms, with or without
# modification, are permitted provided that the following conditions are met:
#
# 1. Redistributions of source code must retain the above copyright notice, this
#    list of conditions and the following disclaimer.
# 2. Redistributions in binary form must reproduce the above copyright notice,
#    this list of conditions and the following disclaimer in the documentation
#    and/or other materials provided with the distribution.
#
# THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND
# ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
# WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
# DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT OWNER OR CONTRIBUTORS BE LIABLE FOR
# ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES
# (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES;
# LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND
# ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
# (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
# SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

import hashlib
import hmac
import importlib
import inspect
import io
import json
import logging
import os
import socket
import ssl
import sys
import threading
import time


PROFILE = "streamlink-8.5.0-drain-v1"
MAX_FRAME = 4096
SOURCES = {
    "streamlink_cli.main": "d4917b9e33675f0e377d0ca2e9e7a5bfc3d84edbec694f98252331db0190b87e",
    "streamlink_cli.streamrunner": "783d38d2bdf23d05b76331e2244bb4710d700ad8ddc557578ab104af1060b622",
    "streamlink.stream.segmented.segmented": "1e48b04bd334393d8db37947e3e3a4909d367383d5a0911d1704f6947a4473b0",
    "streamlink.stream.hls.hls": "9dbed371a67cf528c9b730255996df952833b331297e03a0f26a9f95871f01d8",
    "streamlink.stream.dash.dash": "54c18528e522b71176e726c79b66e1ba6154009a9c67ca545d3529a229b2f5f7",
    "streamlink.stream.filtered": "dcaa44cfb79104faa53862baf7eeeb4569b0902b5e8c774583aff61196b2f322",
    "streamlink.stream.http": "ddf1b5ebffa2ffa28e6c5a05ff6163fb6046060f27d66ace5d92492a982e97dd",
    "streamlink.stream.file": "b8ae0644788b9f6e517dcc002a735a61ad11af38edd85254d2e4d5b88151d187",
    "streamlink.stream.wrappers": "9ad0a2626f5da43fa4befeec3d33610c0a4a37f0001751f3eedf61255a9dcebd",
    "streamlink.stream.ffmpegmux": "b7320e0c843fb4fcec9d92de83ab0cf6812445f31f2c50bb114a48e1e6f9a2f4",
    "streamlink.buffers": "70acbc7995f8128bc4e0a19326718ad084e1913b6b5e3277494d2cae398b5744",
    "streamlink.utils.named_pipe": "b796151b90321d4014e1b70efed559d96665854a64eec448b66a76a44307324f",
    "streamlink.plugins.twitcasting": "2fa50961db2077a5796e7f42e7bf6ac46a14eb10c70cd1763db13254c0861abd",
    "streamlink.plugin.api.websocket": "ce51bef4b41f66fd79480a4ee03160789075ceb1bd3b2529c7b57d49ee66f26c",
    "streamlink.plugins.nicolive": "d371c5f40d3b68336916629b10eb33db5bcf6b64c5d8ce92d51d4b5072f7bc46",
    "streamlink.plugins.ustreamtv": "dd1fa72d1616148dafc0623898109bb4038bf92411a31d71f2a5fd9f6bdf9d51",
    "streamlink.utils.processoutput": "2b2b9dd63600a3775df9801cb27a44780248c6b62dac716924762b5da2d447be",
}

HTTP_SOURCES = {
    "urllib3.response": "f525f806491da0b82cc759deea1a6dfb93e772b9c3ccfcdea82d4369285273e3",
    "requests.models": "d1bc0d990abf5d5ebee05f890911b4363fadf2d5264b686a963df47c529b6ace",
}
WEBSOCKET_SOURCES = {
    "websocket._core": "3011b43e249623594045c808e7fb847aea938a910993eeea85fe7e6ac83f45de",
    "websocket._app": "04b2c6d431480c6c10282db90f5f08ba6399af73155ece346cd8a1148045a1ff",
    "websocket._abnf": "77b1007578230bf99bc82184f9e83014f852ec7b6aa3c38364668d3a4fcaa4cb",
    "websocket._socket": "0e1bf0eba2a9c26eae1ef35c3b71dd4511d0921a97047c15746405f7e150aa6a",
    "websocket._dispatcher": "2fe3bd65d76f3f282356e85924c133a5c3c27a640710a06185c9cd2fed4a5c0f",
}


def audited_modules():
    modules = {}
    for name, expected in SOURCES.items():
        module = importlib.import_module(name)
        source = inspect.getsource(module).replace("\r\n", "\n").encode("utf-8")
        if hashlib.sha256(source).hexdigest() != expected:
            raise ValueError("Streamlink source profile does not match")
        modules[name] = module
    return modules


class Protocol:
    def __init__(self, connection, token):
        self.connection = connection
        self.token = token
        self.lock = threading.Lock()

    def send(self, event, reason=""):
        frame = json.dumps({"token": self.token, "protocol": 1, "profile": PROFILE,
                            "event": event, "reason": reason[:256]}).encode() + b"\n"
        with self.lock:
            self.connection.sendall(frame)

    def receive_stop(self):
        # One command per process; bounded framing also rejects trailing commands.
        data = bytearray()
        while len(data) < MAX_FRAME:
            byte = self.connection.recv(1)
            if not byte:
                raise OSError("control channel closed")
            if byte == b"\n":
                message = json.loads(data)
                if (not isinstance(message, dict) or message.get("protocol") != 1 or message.get("command") != "stop"
                        or not hmac.compare_digest(str(message.get("token", "")), self.token)):
                    raise ValueError("invalid stop command")
                milliseconds = message.get("budget_ms")
                if type(milliseconds) is not int or not 0 <= milliseconds <= 86400000:
                    raise ValueError("invalid stop deadline")
                return milliseconds / 1000
            data.extend(byte)
        raise ValueError("oversized control frame")


class HTTPBody:
    """Gate socket acquisition below existing HTTP/decoder buffering.

    read1 drains http.client's prefetch and urllib3's decoder buffers before
    asking SocketIO for more input. Returning EOF at that lower boundary cannot
    discard the unread suffix of a previous iter_content() call.
    """
    def __init__(self, response, stopped):
        import http.client
        import urllib3.response
        for name, expected in HTTP_SOURCES.items():
            source = inspect.getsource(importlib.import_module(name)).replace("\r\n", "\n").encode()
            if hashlib.sha256(source).hexdigest() != expected:
                raise ValueError("HTTP source profile does not match")
        raw = response.raw
        if (sys.implementation.name != "cpython" or sys.version_info[:2] not in ((3, 11), (3, 14))
                or type(raw) is not urllib3.response.HTTPResponse
                or type(raw._fp) is not http.client.HTTPResponse
                or type(raw._fp.fp) is not io.BufferedReader
                or type(raw._fp.fp.raw) is not socket.SocketIO
                or type(raw._fp.fp.raw._sock) not in (socket.socket, ssl.SSLSocket)
                or raw._decoder is not None or raw._has_decoded_content or len(raw._decoded_buffer)):
            raise ValueError("unsupported HTTP transport or preconsumed decoder")
        # Brotli/Zstandard depend on optional decoder implementations outside the
        # audited response profile. They continue through the original CLI path.
        encodings = response.headers.get("Content-Encoding", "identity").lower().split(",")
        if any(encoding.strip() not in ("identity", "gzip", "x-gzip", "deflate") for encoding in encodings):
            raise ValueError("unsupported HTTP content decoder")
        self.response = response
        self.raw = raw
        self.stopped = stopped
        self.exhausted = False
        self.cutoff = False
        self.closed = False
        self.lock = threading.Lock()
        transport = raw._fp.fp.raw
        original_readinto = transport.readinto
        sock = transport._sock
        def readinto(buffer):
            if stopped.is_set():
                pending = sock.pending() if type(sock) is ssl.SSLSocket else 0
                if pending:
                    return original_readinto(memoryview(buffer)[:pending])
                self.cutoff = True
                # A requested acquisition boundary intentionally shortens a
                # Content-Length body; genuine EOF before stop remains an error.
                raw.enforce_content_length = False
                return 0
            return original_readinto(buffer)
        transport.readinto = readinto

    def __iter__(self):
        return self

    def __next__(self):
        from urllib3.exceptions import ProtocolError
        with self.lock:
            if self.exhausted:
                raise StopIteration
            try:
                data = self.raw.read1(8192, decode_content=True)
            except ProtocolError:
                if not self.cutoff or not self.stopped.is_set():
                    raise OSError("HTTP body ended unexpectedly") from None
                # Chunk framing can reject our deliberate transport EOF after
                # all payload bytes in the existing BufferedReader were read.
                # Drain the existing decoder too; never swallow decoder failures.
                data = self.raw._decoded_buffer.get_all() + self.raw._flush_decoder()
                self.exhausted = True
            except Exception:
                raise OSError("HTTP body decoding or acquisition failed") from None
            if not data:
                self.exhausted = True
                raise StopIteration
            return data

    def close(self):
        with self.lock:
            if not self.closed:
                self.closed = True
                self.response.close()


class FileGate:
    """Unbuffered finite input: bytes beyond the current read are not acquired."""
    def __init__(self, fd, stopped):
        self.fd = fd
        self.stopped = stopped

    def read(self, size=-1):
        # A BytesIO's entire contents have already been acquired. A raw file's
        # unread on-disk suffix has not. Buffered/opaque file objects never use
        # this adapter, because their accepted prefetch is not observable.
        if self.stopped.is_set() and type(self.fd) is not io.BytesIO:
            return b""
        return self.fd.read(size)

    def close(self):
        self.fd.close()

    def __getattr__(self, name):
        return getattr(self.fd, name)


class WebSocketAcquisition:
    """An admitted message spans recv_data_frame through its on_data write.

    A stop cannot close an in-flight receive or discard a frame between receive
    and callback. Existing frame/TLS buffers admit their remaining message too.
    """
    def __init__(self, client, controller):
        import websocket
        for name, expected in WEBSOCKET_SOURCES.items():
            source = inspect.getsource(importlib.import_module(name)).replace("\r\n", "\n").encode()
            if hashlib.sha256(source).hexdigest() != expected:
                raise ValueError("websocket source profile does not match")
        if type(client.ws) is not websocket.WebSocketApp:
            raise ValueError("opaque websocket application")
        self.client = client
        self.owner = controller
        self.condition = threading.Condition()
        self.active = False
        self.closing = False
        self.core = None
        self.supported = True
        acquisition = self
        class ControlledApp(websocket.WebSocketApp):
            def __setattr__(app, name, value):
                if name == "sock" and value is not None:
                    acquisition.attach(value)
                super().__setattr__(name, value)
        client.ws.__class__ = ControlledApp

    def buffered(self, core):
        if core.frame_buffer.recv_buffer or core.cont_frame.cont_data:
            return True
        sock = core.sock
        if sock is None:
            return False
        if type(sock) is ssl.SSLSocket:
            return sock.pending() > 0
        if type(sock) is not socket.socket:
            self.supported = False
            self.owner.fail("opaque websocket transport")
        return False

    def attach(self, core):
        import websocket
        if type(core) is not websocket.WebSocket:
            self.supported = False
            self.owner.fail("opaque websocket frame reader")
            return
        self.core = core
        receive = core.recv_data_frame
        def recv_data_frame(*args, **kwargs):
            with self.condition:
                if self.closing or (self.owner.stopped.is_set() and not self.buffered(core)):
                    return websocket.ABNF.OPCODE_CLOSE, websocket.ABNF.create_frame(
                        b"", websocket.ABNF.OPCODE_CLOSE)
                self.active = True
            try:
                result = receive(*args, **kwargs)
            except BaseException:
                self.delivered()
                raise
            if result[0] not in (websocket.ABNF.OPCODE_TEXT, websocket.ABNF.OPCODE_BINARY,
                                  websocket.ABNF.OPCODE_CONT):
                self.delivered()
            # Media remains owned until the corresponding on_data returns.
            return result
        core.recv_data_frame = recv_data_frame

    def delivered(self):
        with self.condition:
            self.active = False
            self.condition.notify_all()

    def stop(self, reader):
        while self.owner.remaining() > 0:
            with self.condition:
                if self.active or (not self.closing and self.core is not None and self.buffered(self.core)):
                    self.condition.wait(min(0.05, self.owner.remaining()))
                    continue
                self.closing = True
            # Also covers a callback already in progress when control attached.
            with self.client._srec_write_lock:
                self.client.ws.close(timeout=min(0.1, self.owner.remaining()))
            if not self.client.is_alive():
                reader.buffer.close()
                return
            self.client.join(timeout=min(0.05, self.owner.remaining()))
        self.owner.fail("websocket acquisition did not settle before its deadline")


def write_all(data, write):
    offset = 0
    while offset < len(data):
        written = write(data[offset:])
        if not isinstance(written, int) or not 0 < written <= len(data) - offset:
            raise OSError("incomplete muxer pipe write")
        offset += written
    return offset


def flush_before_disconnect(flush, disconnect):
    # Windows disconnect discards unread bytes. Flush blocks until the client
    # reads them; the owning Rust process tree still enforces the hard deadline.
    flush()
    disconnect()


class Controller:
    def __init__(self, protocol, modules):
        self.protocol = protocol
        self.modules = modules
        self.stopped = threading.Event()
        self.lock = threading.RLock()
        self.nodes = {}
        self.root = None
        self.deadline = None
        self.failure = None
        self.opening = False
        self.original = {}

    def remaining(self):
        if self.deadline is None:
            return None
        return max(0, self.deadline - time.monotonic())

    def fail(self, reason):
        with self.lock:
            if self.failure is not None:
                return
            self.failure = reason
        try:
            self.protocol.send("incomplete", reason)
        except OSError:
            pass  # A disconnected owner cannot accept an outcome; never claim success.

    def request_stop(self, budget):
        with self.lock:
            if self.stopped.is_set():
                return
            self.deadline = time.monotonic() + budget
            self.stopped.set()
            nodes = list(self.nodes.values())
        for node, kind in nodes:
            self.stop_node(node, kind)

    def stop_node(self, node, kind):
        if kind == "segmented":
            node.worker.close()  # FIFO sentinel follows accepted segment/map jobs.
            if hasattr(node, "resume"):
                node.resume()
        elif kind == "websocket":
            node._srec_stop_thread = threading.Thread(
                target=node.wsclient._srec_acquisition.stop, args=(node,),
                daemon=True, name="srec-websocket-stop")
            node._srec_stop_thread.start()

    def register(self, node):
        if isinstance(node, RootIO):
            node = node.fd
        with self.lock:
            if id(node) in self.nodes:
                return
            wrappers = self.modules["streamlink.stream.wrappers"]
            segmented = self.modules["streamlink.stream.segmented.segmented"]
            muxer = self.modules["streamlink.stream.ffmpegmux"].FFMPEGMuxer
            websocket = self.modules["streamlink.plugins.twitcasting"].TwitCastingReader
            children = []
            if isinstance(node, segmented.SegmentedStreamReader):
                # Custom fetching/decryption is preserved. Custom lifecycle methods
                # need their own adapter; structural resemblance is not sufficient.
                filtered = self.modules["streamlink.stream.filtered"].FilteredStream
                closes = (segmented.SegmentedStreamReader.close, filtered.close,
                          self.modules["streamlink.plugins.nicolive"].NicoLiveHLSStreamReader.close,
                          self.modules["streamlink.plugins.ustreamtv"].UStreamTVStreamReader.close)
                if (type(node).read not in (segmented.SegmentedStreamReader.read, filtered.read)
                        or type(node).close not in closes
                        or type(node.worker).run is not segmented.SegmentedStreamWorker.run
                        or type(node.worker).close is not segmented.SegmentedStreamWorker.close
                        or type(node.writer).run is not segmented.SegmentedStreamWriter.run
                        or type(node.writer).close is not segmented.SegmentedStreamWriter.close):
                    self.fail("unsupported segmented lifecycle override")
                    return
                kind = "segmented"
                worker_types = (self.modules["streamlink.stream.hls.hls"].HLSStreamWorker,
                                self.modules["streamlink.stream.dash.dash"].DASHStreamWorker)
                for worker_type in worker_types:
                    if isinstance(node.worker, worker_type):
                        if type(node.worker).reload is not worker_type.reload:
                            self.fail("unsupported manifest reload override")
                            return
                        wait_free = node.buffer.wait_free
                        def wait_for_acquisition(*args, **kwargs):
                            result = wait_free(*args, **kwargs)
                            if self.stopped.is_set():
                                raise AcquisitionStopped
                            return result
                        node.buffer.wait_free = wait_for_acquisition
                        break
            elif type(node) is muxer:
                kind, children = "muxer", node.streams
            elif type(node) is wrappers.StreamIOThreadWrapper:
                kind, children = "thread", [node.fd]
            elif type(node) is wrappers.StreamIOIterWrapper:
                if type(node.iterator) is not HTTPBody:
                    self.fail("unsupported or opaque HTTP iterator")
                    return
                kind = "iterator"
            elif type(node) is wrappers.StreamIOWrapper:
                kind, children = "wrapper", [node.fd]
            elif type(node) is websocket:
                if node.wsclient._srec_acquisition is None:
                    self.fail("unsupported websocket acquisition profile")
                    return
                kind = "websocket"
            elif type(node) is FileGate and type(node.fd) in (io.FileIO, io.BytesIO):
                kind = "file"
            else:
                self.fail("unsupported stream reader: " + type(node).__name__[:80])
                return
            self.nodes[id(node)] = (node, kind)
            for child in children:
                self.register(child)
            stopped = self.stopped.is_set()
        if stopped:
            self.stop_node(node, kind)

    def settle(self):
        for node, kind in list(self.nodes.values()):
            threads = []
            if kind == "segmented":
                threads = [node.worker, node.writer]
            elif kind == "thread":
                threads = [node.filler]
            elif kind == "websocket":
                threads = [node.wsclient]
                if hasattr(node, "_srec_stop_thread"):
                    threads.append(node._srec_stop_thread)
            elif kind == "muxer":
                threads = node.pipe_threads
            for thread in threads:
                if thread is not threading.current_thread() and thread.is_alive():
                    thread.join(self.remaining())
                if thread.is_alive():
                    raise OSError("producer did not settle before drain deadline")
            if kind in ("segmented", "thread", "iterator", "websocket") and node.buffer.length:
                raise OSError("reader buffer was not exhausted")
            if kind in ("segmented", "thread", "websocket") and not node.buffer.closed:
                raise OSError("producer did not close its buffer")
            if kind == "iterator" and not node.iterator.exhausted:
                raise OSError("HTTP acquisition did not reach its stop boundary")
            if kind == "thread" and node.filler.error:
                raise OSError("HTTP producer failed")
            if kind == "muxer" and node.process:
                if node.process.wait(timeout=self.remaining()) != 0:
                    raise OSError("internal muxer failed")

    def finished(self):
        if not self.stopped.is_set():
            return
        if self.root is None or not self.root.eof:
            self.fail("stream output did not reach EOF")
            return
        try:
            self.settle()
            self.modules["streamlink_cli.main"].stdout.flush()
            if self.failure is None:
                self.protocol.send("drained")
        except Exception:
            self.fail("stream drain or output flush failed")


class RootIO:
    def __init__(self, fd, controller):
        self.fd = fd
        self.controller = controller
        self.eof = False

    def read(self, size=-1):
        try:
            data = self.fd.read(size)
            self.eof = data == b""
            return data
        except Exception:
            self.controller.fail("stream read failed")
            raise

    def close(self):
        if self.controller.stopped.is_set() and self.eof:
            try:
                self.controller.settle()
            except Exception:
                self.controller.fail("producer or muxer did not finish")
        self.fd.close()

    def __getattr__(self, name):
        return getattr(self.fd, name)


class AcquisitionStopped(Exception):
    pass


def install(controller):
    modules = controller.modules
    cli = modules["streamlink_cli.main"]
    wrappers = modules["streamlink.stream.wrappers"]
    segmented = modules["streamlink.stream.segmented.segmented"]
    filtered = modules["streamlink.stream.filtered"].FilteredStream
    websocket = modules["streamlink.plugins.twitcasting"]
    original_open = cli.open_stream
    original_runner = cli.StreamRunner
    original_reader_open = segmented.SegmentedStreamReader.open
    original_iter_close = wrappers.StreamIOIterWrapper.close
    original_filtered_read = filtered.read
    original_file_open = modules["streamlink.stream.file"].FileStream.open
    original_ws_init = websocket.TwitCastingWsClient.__init__
    original_ws_data = websocket.TwitCastingWsClient.on_data
    original_ws_open = websocket.TwitCastingReader.open
    original_future = segmented.SegmentedStreamWriter._future_result
    hls_worker = modules["streamlink.stream.hls.hls"].HLSStreamWorker
    dash_worker = modules["streamlink.stream.dash.dash"].DASHStreamWorker
    muxer = modules["streamlink.stream.ffmpegmux"].FFMPEGMuxer
    original_copy = muxer.copy_to_pipe

    def patch(target, name, value):
        controller.original[(target, name)] = inspect.getattr_static(target, name)
        setattr(target, name, value)

    def open_stream(stream):
        if controller.stopped.is_set():
            raise OSError("recording stopped before opening a stream")
        controller.opening = True
        class OpenProxy:
            def open(self):
                fd = stream.open()
                controller.register(fd)
                root = RootIO(fd, controller)
                controller.root = root
                return root
        try:
            return original_open(OpenProxy())
        finally:
            controller.opening = False

    class Runner(original_runner):
        def run(self, *args, **kwargs):
            try:
                super().run(*args, **kwargs)
            except BaseException:
                controller.fail("output runner was interrupted or failed")
                raise
            else:
                controller.finished()

    def reader_open(reader):
        if controller.opening:
            controller.register(reader)
        original_reader_open(reader)

    def iterator_close(reader):
        if type(reader.iterator) is HTTPBody:
            reader.iterator.close()
        original_iter_close(reader)

    def http_open(stream):
        from streamlink.exceptions import StreamError
        request = stream.session.http.valid_request_args(**stream.args)
        request.setdefault("method", "GET")
        timeout = stream.session.options.get("stream-timeout")
        response = stream.session.http.request(stream=True, exception=StreamError, timeout=timeout, **request)
        try:
            iterator = HTTPBody(response, controller.stopped)
        except (ValueError, OSError, AttributeError, TypeError, ImportError):
            iterator = response.iter_content(8192)
            controller.fail("unsupported HTTP transport, interpreter or decoder profile")
        fd = wrappers.StreamIOIterWrapper(iterator)
        return wrappers.StreamIOThreadWrapper(stream.session, fd, timeout=timeout) if stream.buffered else fd

    def filler_run(filler):
        # Adapted from StreamIOThreadWrapper.Filler.run. Keep the current read
        # and write alive; the iterator gate turns subsequent acquisition into
        # EOF, letting both the iterator buffer and the ring buffer empty.
        filler.running = True
        while filler.running:
            try:
                data = filler.fd.read(8192)
            except OSError as error:
                filler.error = error
                controller.fail("HTTP producer read failed")
                break
            if not data:
                break
            filler.buffer.write(data)
        filler.stop()

    def filtered_read(reader, *args, **kwargs):
        if (not isinstance(reader, segmented.SegmentedStreamReader)
                or getattr(super(filtered, reader).read, "__func__", None)
                is not segmented.SegmentedStreamReader.read):
            return original_filtered_read(reader, *args, **kwargs)
        # Adapted from FilteredStream.read: a stop must wake a read that entered
        # the filtering wait before the control request, and exhaust buffered
        # data even if the writer closed while that read was parked.
        while True:
            try:
                return segmented.SegmentedStreamReader.read(reader, *args, **kwargs)
            except OSError:
                while not reader._event_filter.wait(0.1):
                    if controller.stopped.is_set():
                        break
                if reader.buffer.length:
                    continue
                if reader.buffer.closed:
                    return b""
                if controller.stopped.is_set() and controller.remaining() > 0:
                    continue  # an admitted fetch may still deliver its tail
                raise

    def buffered_read(original, producer):
        def read(reader, size=-1):
            if not controller.stopped.is_set():
                try:
                    return original(reader, size)
                except OSError as error:
                    if not controller.stopped.is_set() or str(error) != "Read timeout":
                        raise
            return reader.buffer.read(size, block=producer(reader).is_alive(),
                                      timeout=controller.remaining())
        return read

    def copy_to_pipe(mux, stream, pipe):
        class CheckedPipe:
            def __getattr__(self, name):
                return getattr(pipe, name)

            def write(self, data):
                try:
                    return write_all(data, pipe.write)
                except OSError:
                    controller.fail("internal muxer pipe write failed")
                    raise

            def close(self):
                try:
                    pipe.close()
                except OSError:
                    controller.fail("internal muxer pipe flush or close failed")
                    raise
        original_copy(mux, stream, CheckedPipe())

    def ws_init(client, *args, **kwargs):
        client._srec_write_lock = threading.Lock()
        client._srec_acquisition = None
        original_ws_init(client, *args, **kwargs)
        try:
            client._srec_acquisition = WebSocketAcquisition(client, controller)
        except (ValueError, TypeError, OSError, AttributeError, ImportError):
            controller.fail("unsupported websocket acquisition profile")

    def ws_data(client, *args, **kwargs):
        try:
            with client._srec_write_lock:
                return original_ws_data(client, *args, **kwargs)
        finally:
            if client._srec_acquisition is not None:
                client._srec_acquisition.delivered()

    def ws_open(reader):
        # Admission and Thread.start are serialized with the stop latch. Once
        # admitted, the acquisition owner keeps closing until startup has seen
        # the stop; run_forever cannot undo a one-shot pre-start close.
        with controller.lock:
            if controller.stopped.is_set():
                reader.buffer.close()
                controller.register(reader)
                return
            controller.register(reader)
            original_ws_open(reader)

    def future_result(future):
        result = original_future(future)
        if result is None:
            controller.fail("an admitted segment could not be fetched")
        return result

    def guarded_reload(original):
        def reload(worker):
            try:
                return original(worker)
            except AcquisitionStopped:
                return None
        return reload

    def file_open(stream):
        if stream.fileobj is not None and type(stream.fileobj) is not io.BytesIO:
            return original_file_open(stream)
        fd = stream.fileobj if stream.fileobj is not None else stream.path.open("rb", buffering=0)
        return FileGate(fd, controller.stopped)

    class ProducerErrors(logging.Handler):
        def emit(self, record):
            if record.levelno >= logging.ERROR:
                controller.fail("Streamlink reported an acquisition or output error")

    patch(cli, "open_stream", open_stream)
    patch(cli, "StreamRunner", Runner)
    patch(segmented.SegmentedStreamReader, "open", reader_open)
    patch(segmented.SegmentedStreamWriter, "_future_result", staticmethod(future_result))
    patch(hls_worker, "reload", guarded_reload(hls_worker.reload))
    patch(dash_worker, "reload", guarded_reload(dash_worker.reload))
    patch(wrappers.StreamIOIterWrapper, "close", iterator_close)
    patch(wrappers.StreamIOThreadWrapper.Filler, "run", filler_run)
    patch(filtered, "read", filtered_read)
    patch(segmented.SegmentedStreamReader, "read", buffered_read(
        segmented.SegmentedStreamReader.read, lambda reader: reader.writer))
    patch(wrappers.StreamIOThreadWrapper, "read", buffered_read(
        wrappers.StreamIOThreadWrapper.read, lambda reader: reader.filler))
    patch(websocket.TwitCastingWsClient, "__init__", ws_init)
    patch(websocket.TwitCastingWsClient, "on_data", ws_data)
    patch(websocket.TwitCastingReader, "open", ws_open)
    patch(websocket.TwitCastingReader, "read", buffered_read(
        websocket.TwitCastingReader.read, lambda reader: reader.wsclient))
    patch(modules["streamlink.stream.file"].FileStream, "open", file_open)
    patch(modules["streamlink.stream.http"].HTTPStream, "open", http_open)
    patch(muxer, "copy_to_pipe", staticmethod(copy_to_pipe))
    logging.getLogger("streamlink").addHandler(ProducerErrors())
    if os.name == "nt":
        install_windows(modules, patch)


def install_windows(modules, patch):
    import ctypes
    from ctypes import wintypes
    mux = modules["streamlink.stream.ffmpegmux"]
    pipe_type = modules["streamlink.utils.named_pipe"].NamedPipeWindows
    kernel = ctypes.windll.kernel32
    kernel.CreateNamedPipeW.restype = wintypes.HANDLE
    patch(pipe_type, "INVALID_HANDLE_VALUE", ctypes.c_void_p(-1).value)
    kernel.CreateNamedPipeW.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD,
        wintypes.DWORD, wintypes.DWORD, wintypes.DWORD, wintypes.DWORD, ctypes.c_void_p]
    kernel.ConnectNamedPipe.argtypes = [wintypes.HANDLE, ctypes.c_void_p]
    kernel.WriteFile.argtypes = [wintypes.HANDLE, ctypes.c_void_p, wintypes.DWORD,
                                ctypes.POINTER(wintypes.DWORD), ctypes.c_void_p]
    for name in ("FlushFileBuffers", "DisconnectNamedPipe", "CloseHandle"):
        getattr(kernel, name).argtypes = [wintypes.HANDLE]

    def checked(success):
        if not success:
            raise OSError("muxer pipe operation failed", kernel.GetLastError())

    def pipe_open(pipe):
        result = kernel.ConnectNamedPipe(pipe.pipe, None)
        if not result and kernel.GetLastError() != 535:  # ERROR_PIPE_CONNECTED
            checked(result)
        pipe._srec_connected = True

    def pipe_write(pipe, data):
        def write(chunk):
            count = wintypes.DWORD()
            checked(kernel.WriteFile(pipe.pipe, ctypes.c_char_p(chunk), len(chunk),
                                     ctypes.byref(count), None))
            return count.value
        return write_all(data, write)

    def pipe_close(pipe):
        if pipe.pipe is None:
            return
        try:
            if getattr(pipe, "_srec_connected", False):
                flush_before_disconnect(lambda: checked(kernel.FlushFileBuffers(pipe.pipe)),
                                        lambda: checked(kernel.DisconnectNamedPipe(pipe.pipe)))
        finally:
            checked(kernel.CloseHandle(pipe.pipe))
            pipe.pipe = None

    original_subprocess = mux.subprocess
    class HiddenMuxerProcesses:
        def __getattr__(self, name):
            return getattr(original_subprocess, name)

        def Popen(self, *args, **kwargs):
            kwargs["creationflags"] = kwargs.get("creationflags", 0) | 0x08000000
            return original_subprocess.Popen(*args, **kwargs)

    patch(pipe_type, "open", pipe_open)
    patch(pipe_type, "write", pipe_write)
    patch(pipe_type, "close", pipe_close)
    patch(mux, "subprocess", HiddenMuxerProcesses())
    # FFmpeg's validation runs through trio, not ffmpegmux.subprocess. Keep
    # validation enabled and hide that process without changing global trio.
    output = modules["streamlink.utils.processoutput"]
    original_trio = output.trio
    class HiddenValidationProcesses:
        def __getattr__(self, name):
            return getattr(original_trio, name)

        async def run_process(self, *args, **kwargs):
            kwargs["creationflags"] = kwargs.get("creationflags", 0) | 0x08000000
            return await original_trio.run_process(*args, **kwargs)
    patch(output, "trio", HiddenValidationProcesses())


def bootstrap():
    from streamlink.plugin import Plugin
    global __plugin__
    class RecordingControl(Plugin):
        def _get_streams(self):
            return iter(())
    __plugin__ = RecordingControl
    token = os.environ.get("SREC_STREAMLINK_CONTROL_TOKEN", "")
    port = os.environ.get("SREC_STREAMLINK_CONTROL_PORT", "")
    if len(token) != 64 or not port.isdecimal():
        return
    try:
        connection = socket.create_connection(("127.0.0.1", int(port)), timeout=2)
        protocol = Protocol(connection, token)
    except OSError:
        return  # Recording keeps its existing CLI behavior without a control owner.
    try:
        controller = Controller(protocol, audited_modules())
        install(controller)
        protocol.send("ready")
    except Exception:
        try:
            protocol.send("unsupported", "Streamlink source profile unavailable or changed")
        finally:
            connection.close()
        return

    def listen():
        try:
            connection.settimeout(None)
            controller.request_stop(protocol.receive_stop())
        except Exception:
            controller.fail("control channel was lost or invalid")
    threading.Thread(target=listen, daemon=True, name="srec-recording-control").start()


if os.environ.get("SREC_STREAMLINK_CONTROL_TOKEN"):
    bootstrap()
