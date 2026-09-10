"""Offline tests against the actual audited Streamlink classes. No child processes."""
import importlib.util
import inspect
import io
import json
import http.client
import logging
from pathlib import Path
import socket
import ssl
import tempfile
import threading
import time
import types
import unittest
from unittest import mock

from streamlink import Streamlink

SOURCE = Path(__file__).resolve().parents[2] / "src/downloader/engine/streamlink/srec_control.py"
SPEC = importlib.util.spec_from_file_location("srec_control_test", SOURCE)
control = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(control)


class Messages:
    def __init__(self):
        self.events = []

    def send(self, event, reason=""):
        self.events.append((event, reason))


class Output:
    def __init__(self):
        self.data = bytearray()

    def write(self, data):
        self.data.extend(data)


def tls_certificate(directory):
    """Ephemeral test certificate using Streamlink's crypto dependency; no openssl."""
    import base64
    from Crypto.PublicKey import RSA
    from Crypto.Hash import SHA256
    from Crypto.Signature import pkcs1_15
    from Crypto.Util.asn1 import DerBitString, DerInteger, DerNull, DerObjectId, DerSequence, DerSetOf
    def sequence(*values):
        return DerSequence(list(values)).encode()
    key = RSA.generate(2048)
    algorithm = sequence(DerObjectId("1.2.840.113549.1.1.11").encode(), DerNull().encode())
    name = sequence(DerSetOf([sequence(DerObjectId("2.5.4.3").encode(), b"\x0c\x09localhost")]).encode())
    validity = sequence(b"\x17\x0d240101000000Z", b"\x17\x0d490101000000Z")
    body = sequence(DerInteger(1).encode(), algorithm, name, validity, name, key.public_key().export_key("DER"))
    certificate = sequence(body, algorithm, DerBitString(pkcs1_15.new(key).sign(SHA256.new(body))).encode())
    encoded = base64.encodebytes(certificate)
    cert, private = Path(directory) / "certificate.pem", Path(directory) / "private.pem"
    cert.write_bytes(b"-----BEGIN CERTIFICATE-----\n" + encoded + b"-----END CERTIFICATE-----\n")
    private.write_bytes(key.export_key("PEM"))
    return cert, private


class CompanionTests(unittest.TestCase):
    def setUp(self):
        self.modules = control.audited_modules()
        self.messages = Messages()
        self.owner = control.Controller(self.messages, self.modules)
        self.session = Streamlink()
        self.session.set_option("ringbuffer-size", 32)
        self.session.set_option("stream-timeout", 0.1)
        self.session.set_option("stream-segment-threads", 1)
        self.session.set_option("stream-segment-attempts", 1)
        self.seg = self.modules["streamlink.stream.segmented.segmented"]
        self.wrap = self.modules["streamlink.stream.wrappers"]
        self.cli = self.modules["streamlink_cli.main"]
        self.websocket = self.modules["streamlink.plugins.twitcasting"]
        targets = [
            (self.cli, "open_stream"), (self.cli, "StreamRunner"),
            (self.seg.SegmentedStreamReader, "open"),
            (self.seg.SegmentedStreamWriter, "_future_result"),
            (self.wrap.StreamIOIterWrapper, "__init__"),
            (self.wrap.StreamIOIterWrapper, "close"),
            (self.wrap.StreamIOThreadWrapper.Filler, "run"),
            (self.modules["streamlink.stream.filtered"].FilteredStream, "read"),
            (self.websocket.TwitCastingWsClient, "__init__"),
            (self.websocket.TwitCastingWsClient, "on_data"),
            (self.websocket.TwitCastingReader, "open"),
            (self.modules["streamlink.stream.file"].FileStream, "open"),
            (self.modules["streamlink.stream.ffmpegmux"].FFMPEGMuxer, "copy_to_pipe"),
        ]
        self.saved = [(target, name, inspect.getattr_static(target, name)) for target, name in targets]
        self.handlers = list(logging.getLogger("streamlink").handlers)
        self.connections = []
        with mock.patch.object(control, "install_windows"):
            control.install(self.owner)

    def tearDown(self):
        for (target, name), value in self.owner.original.items():
            setattr(target, name, value)
        for target, name, value in self.saved:
            setattr(target, name, value)
        logging.getLogger("streamlink").handlers[:] = self.handlers
        for connection in self.connections:
            connection.close()

    def drain(self, reader, prebuffer=b""):
        self.owner.register(reader)
        root = control.RootIO(reader, self.owner)
        self.owner.root = root
        output = Output()
        self.cli.StreamRunner(root, output).run(prebuffer)
        return bytes(output.data)

    def wait_full(self, buffer):
        deadline = time.monotonic() + 2
        while not buffer.is_full:
            self.assertLess(time.monotonic(), deadline, "fixture producer did not fill the ring")
            time.sleep(0.001)

    def assert_drained(self):
        self.assertIn(("drained", ""), self.messages.events, self.messages.events)
        self.assertIsNone(self.owner.failure)

    def segmented(self, chunks, reader=None):
        stream = types.SimpleNamespace(session=self.session, args={})
        reader = reader or self.seg.SegmentedStreamReader(stream)
        reader.writer.fetch = lambda segment: chunks[segment.num]
        reader.writer.write = lambda segment, data, *extra: reader.buffer.write(data)
        queued = threading.Event()
        def segments():
            for index in range(len(chunks)):
                yield types.SimpleNamespace(num=index, duration=1)
            queued.set()
            reader.worker.wait(5)
        reader.worker.iter_segments = segments
        self.owner.opening = True
        reader.open()
        self.owner.opening = False
        self.assertTrue(queued.wait(2))
        self.wait_full(reader.buffer)
        return reader

    def test_segmented_stop_drains_full_ring_and_all_admitted_jobs(self):
        chunks = [bytes([i]) * 97 for i in range(4)]
        reader = self.segmented(chunks)
        prebuffer = reader.read(11)
        self.owner.request_stop(2)
        self.assertEqual(self.drain(reader, prebuffer), b"".join(chunks))
        self.assert_drained()

    def test_latched_stop_applies_to_a_reader_opened_during_startup(self):
        reader = self.seg.SegmentedStreamReader(types.SimpleNamespace(session=self.session))
        reader.worker.iter_segments = lambda: iter(())
        self.owner.request_stop(2)
        self.owner.opening = True
        reader.open()
        self.assertEqual(self.drain(reader), b"")
        self.assert_drained()

    def test_filtered_reader_already_parked_wakes_and_drains(self):
        hls = self.modules["streamlink.stream.hls.hls"]
        reader = hls.HLSStreamReader(hls.HLSStream(self.session, "https://invalid.test/live.m3u8"))
        reader.writer.start()
        reader.pause()
        output = []
        entered = threading.Event()
        def read():
            entered.set()
            output.append(reader.read(8192))
        thread = threading.Thread(target=read, daemon=True)
        thread.start()
        self.assertTrue(entered.wait(1))
        time.sleep(0.15)  # enter the actual FilteredStream timeout/filter wait
        reader.buffer.write(b"accepted tail")
        reader.buffer.close()
        self.owner.register(reader)
        self.owner.request_stop(2)
        reader.writer.put(None)
        thread.join(2)
        self.assertFalse(thread.is_alive())
        self.assertEqual(output, [b"accepted tail"])

    def test_opaque_filler_keeps_recording_without_claiming_an_acquisition_boundary(self):
        accepted = b"first" * 3200
        acquisitions = []
        def source():
            acquisitions.append(1)
            yield accepted
            acquisitions.append(2)
            yield b"future"
        iterator = self.wrap.StreamIOIterWrapper(source())
        reader = self.wrap.StreamIOThreadWrapper(self.session, iterator, timeout=1)
        self.owner.register(reader)
        self.wait_full(reader.buffer)
        self.owner.request_stop(2)
        self.assertEqual(self.drain(reader), accepted + b"future")
        self.assertEqual(acquisitions, [1, 2])
        self.assertNotIn(("drained", ""), self.messages.events)

    def test_opaque_wrapper_keeps_custom_iteration_semantics(self):
        entered, release = threading.Event(), threading.Event()
        def source():
            entered.set()
            self.assertTrue(release.wait(2))
            yield b"accepted"
            yield b"future"
        reader = self.wrap.StreamIOWrapper(self.wrap.StreamIOIterWrapper(source()))
        result = []
        thread = threading.Thread(target=lambda: result.append(self.drain(reader)), daemon=True)
        thread.start()
        self.assertTrue(entered.wait(1))
        self.owner.request_stop(2)
        release.set()
        thread.join(2)
        self.assertFalse(thread.is_alive())
        self.assertEqual(result, [b"acceptedfuture"])
        self.assertNotIn(("drained", ""), self.messages.events)

    def test_file_prebuffer_is_kept_without_acquiring_the_rest_of_the_file(self):
        file_stream = self.modules["streamlink.stream.file"].FileStream
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "input"
            path.write_bytes(b"prebuffer-future")
            reader = file_stream(self.session, path=path).open()
            prebuffer = reader.read(9)
            self.owner.request_stop(2)
            self.assertEqual(self.drain(reader, prebuffer), b"prebuffer")
            self.assert_drained()

    def test_actual_cli_open_stream_prebuffer_is_forwarded_once(self):
        file_stream = self.modules["streamlink.stream.file"].FileStream
        payload = b"cli-prebuffer-tail" * 2000
        fd, prebuffer = self.cli.open_stream(file_stream(self.session, fileobj=io.BytesIO(payload)))
        self.assertEqual(prebuffer, payload[:8192])
        self.owner.request_stop(2)
        output = Output()
        self.cli.StreamRunner(fd, output).run(prebuffer)
        self.assertEqual(output.data, payload)
        self.assert_drained()

    def test_stop_during_graph_opening_reaches_an_already_opened_child(self):
        queued, release = threading.Event(), threading.Event()
        payloads = [b'a' * 9000, b'b' * 9000]
        reader = self.seg.SegmentedStreamReader(types.SimpleNamespace(session=self.session))
        reader.writer.fetch = lambda segment: payloads[segment.num]
        reader.writer.write = lambda segment, data: reader.buffer.write(data)
        def segments():
            for index in range(2):
                yield types.SimpleNamespace(num=index, duration=1)
            queued.set()
            reader.worker.wait(5)
        reader.worker.iter_segments = segments
        test = self
        class OpeningGraph:
            def open(self):
                reader.open()
                test.assertTrue(release.wait(2))
                return test.wrap.StreamIOWrapper(reader)
        result = []
        def run():
            fd, prebuffer = self.cli.open_stream(OpeningGraph())
            output = Output()
            self.cli.StreamRunner(fd, output).run(prebuffer)
            result.append(bytes(output.data))
        thread = threading.Thread(target=run, daemon=True)
        thread.start()
        self.assertTrue(queued.wait(1))
        self.assertIsNone(self.owner.root)
        self.assertIn(id(reader), self.owner.nodes)
        self.owner.request_stop(2)
        self.assertTrue(reader.worker.closed)
        release.set()
        thread.join(2)
        self.assertFalse(thread.is_alive())
        self.assertEqual(result, [b"".join(payloads)])
        self.assert_drained()

    def test_websocket_stop_waits_for_the_current_full_ring_write(self):
        stream = types.SimpleNamespace(session=self.session, url="wss://invalid.test/")
        reader = self.websocket.TwitCastingReader(stream)
        alive = threading.Event()
        alive.set()
        reader.wsclient.is_alive = lambda: alive.is_set()
        def close(**kwargs):
            alive.clear()
            reader.buffer.close()
        reader.wsclient.ws.close = close
        reader.wsclient.join = lambda timeout=None: None
        self.owner.register(reader)
        payload = b"websocket" * 200
        producer = threading.Thread(target=lambda: reader.wsclient.on_data(None, payload, 2, True), daemon=True)
        producer.start()
        self.wait_full(reader.buffer)
        self.owner.request_stop(2)
        self.assertEqual(self.drain(reader), payload)
        producer.join(2)
        self.assertFalse(producer.is_alive())
        self.assert_drained()

    def test_memory_file_preserves_all_preacquired_bytes(self):
        file_stream = self.modules["streamlink.stream.file"].FileStream
        reader = file_stream(self.session, fileobj=io.BytesIO(b"prebuffer-tail")).open()
        prebuffer = reader.read(9)
        self.owner.request_stop(2)
        self.assertEqual(self.drain(reader, prebuffer), b"prebuffer-tail")
        self.assert_drained()

    def test_opaque_buffered_fileobj_keeps_its_reader_and_refuses_drain_guarantee(self):
        file_stream = self.modules["streamlink.stream.file"].FileStream
        original = io.BufferedReader(io.BytesIO(b"buffered file tail"), buffer_size=16)
        reader = file_stream(self.session, fileobj=original).open()
        self.assertIs(reader, original)
        prebuffer = reader.read(2)
        self.owner.register(reader)
        self.owner.request_stop(2)
        self.assertEqual(self.drain(reader, prebuffer), b"buffered file tail")
        self.assertIn("unsupported stream reader", self.owner.failure)
        self.assertNotIn(("drained", ""), self.messages.events)

    def test_opaque_iterator_finishes_naturally_without_a_drain_claim(self):
        events = []
        def source():
            try:
                yield b"acquired" * 2000
                yield b"not acquired"
            finally:
                events.append("closed")
        reader = self.wrap.StreamIOThreadWrapper(self.session, self.wrap.StreamIOIterWrapper(source()), timeout=1)
        self.owner.register(reader)
        self.wait_full(reader.buffer)
        self.owner.request_stop(2)
        self.assertEqual(self.drain(reader), b"acquired" * 2000 + b"not acquired")
        self.assertEqual(events, ["closed"])
        self.assertNotIn(("drained", ""), self.messages.events)

    def test_custom_unknown_reader_keeps_recording_but_cannot_claim_drain(self):
        reader = io.BytesIO(b"unchanged")
        self.owner.register(reader)
        self.assertIn("unsupported stream reader", self.owner.failure)
        self.owner.request_stop(2)
        self.assertEqual(self.drain(reader), b"unchanged")
        self.assertNotIn(("drained", ""), self.messages.events)

    def test_reader_failure_cannot_report_a_successful_drain(self):
        def source():
            yield b"prefix"
            raise OSError("fixture network error")
        reader = self.wrap.StreamIOIterWrapper(source())
        with self.assertRaises(OSError):
            self.drain(reader)
        self.owner.request_stop(1)
        self.owner.finished()
        self.assertNotIn(("drained", ""), self.messages.events)

    def queue_segments(self, reader, segments):
        queued = threading.Event()
        def sequence():
            for segment in segments:
                yield segment
            queued.set()
            reader.worker.wait(5)
        reader.worker.iter_segments = sequence
        self.owner.opening = True
        reader.open()
        self.owner.opening = False
        self.assertTrue(queued.wait(2))
        self.wait_full(reader.buffer)

    def test_hls_maps_encryption_and_filtered_segments_finish_in_fifo_order(self):
        from Crypto.Cipher import AES
        from Crypto.Util.Padding import pad
        from streamlink.stream.hls.segment import HLSSegment
        from streamlink.stream.hls.m3u8 import Key, Map
        import re
        hls = self.modules["streamlink.stream.hls.hls"]
        reader = hls.HLSStreamReader(hls.HLSStream(self.session, "https://invalid.test/fixture.m3u8"))
        key_bytes, iv = b'k' * 16, b'i' * 16
        key = Key(method="AES-128", uri="https://invalid.test/key", iv=iv,
                  key_format=None, key_format_versions=None)
        init = Map(uri="init", key=None, byterange=None)
        segments = [HLSSegment(num=i, uri=uri, duration=1, title=None, key=key if i == 2 else None,
                              byterange=None, date=None, map=init if i == 0 else None)
                    for i, uri in enumerate(["plain", "ad", "encrypted"])]
        plain, decrypted, prefix = b'p' * 101, b'e' * 109, b'm' * 41
        encrypted = AES.new(key_bytes, AES.MODE_CBC, iv).encrypt(pad(decrypted, 16))
        def response(data):
            return types.SimpleNamespace(content=data, iter_content=lambda size: iter([data]),
                                         raw=types.SimpleNamespace(drain_conn=lambda: None))
        reader.writer.fetch = lambda segment: response({"plain": plain, "ad": b"discard", "encrypted": encrypted}[segment.uri])
        reader.writer.fetch_map = lambda segment: response(prefix)
        reader.writer.create_decryptor = lambda key, num: AES.new(key_bytes, AES.MODE_CBC, iv)
        reader.writer.ignore_names = re.compile("ad")
        self.queue_segments(reader, segments)
        self.owner.request_stop(2)
        self.assertEqual(self.drain(reader), prefix + plain + decrypted)
        self.assert_drained()

    def test_dash_writer_finishes_accepted_response_chunks(self):
        writer_type = self.modules["streamlink.stream.dash.dash"].DASHStreamWriter
        class Reader(self.seg.SegmentedStreamReader):
            __writer__ = writer_type
        reader = Reader(types.SimpleNamespace(session=self.session, args={}))
        reader.mime_type = "video"
        chunks = [b'v' * 103, b'a' * 107]
        reader.writer.fetch = lambda segment: types.SimpleNamespace(
            iter_content=lambda size: iter([chunks[segment.num][:7], chunks[segment.num][7:]]))
        self.queue_segments(reader, [types.SimpleNamespace(num=i, uri=str(i), duration=1, name=str(i)) for i in range(2)])
        self.owner.request_stop(2)
        self.assertEqual(self.drain(reader), b"".join(chunks))
        self.assert_drained()

    def response(self, body, encoding="identity", chunked=False, declared_length=None):
        left, right = socket.socketpair()
        left.settimeout(2)
        right.settimeout(2)
        self.connections.extend([left, right])
        headers = {"Content-Encoding": encoding, "Connection": "keep-alive"}
        if chunked:
            headers["Transfer-Encoding"] = "chunked"
            wire = f"{len(body):x}\r\n".encode() + body + b"\r\n"
            # Keep the connection open without a final chunk. Stop must end
            # acquisition at the socket gate after all prefetched payload.
        else:
            headers["Content-Length"] = str(declared_length or len(body))
            wire = body
        header = b"HTTP/1.1 200 OK\r\n" + b"".join(
            f"{key}: {value}\r\n".encode() for key, value in headers.items()) + b"\r\n"
        right.sendall(header + wire)
        return self.parsed_response(left, headers), right

    def parsed_response(self, left, headers):
        import requests
        import urllib3.response
        parsed = http.client.HTTPResponse(left)
        parsed.begin()
        response = requests.Response()
        response.headers.update(headers)
        response.raw = urllib3.response.HTTPResponse(
            body=parsed, original_response=parsed, headers=headers, preload_content=False,
            enforce_content_length=True, request_method="GET")
        return response

    def test_http_transport_prefetch_and_decoder_tails_drain_after_stop(self):
        import gzip
        import zlib
        for encoding, compress in [("identity", lambda data: data), ("gzip", gzip.compress),
                                   ("deflate", zlib.compress)]:
            for chunked in (False, True):
                with self.subTest(encoding=encoding, chunked=chunked):
                    expected = b"accepted" * (100 if encoding == "identity" else 20000)
                    response, _ = self.response(compress(expected), encoding, chunked)
                    body = control.HTTPBody(response, self.owner.stopped)
                    reader = self.wrap.StreamIOIterWrapper(body)
                    self.owner.request_stop(2)
                    self.assertEqual(self.drain(reader), expected)
                    self.assertTrue(body.closed)
                    self.assert_drained()

    def test_http_inflight_transport_read_finishes_before_the_gate_returns_eof(self):
        response, server = self.response(b"prefix", declared_length=10000)
        transport = response.raw._fp.fp.raw
        readinto = transport.readinto
        entered = threading.Event()
        def read(buffer):
            entered.set()
            return readinto(buffer)
        transport.readinto = read
        body = control.HTTPBody(response, self.owner.stopped)
        reader = self.wrap.StreamIOThreadWrapper(
            self.session, self.wrap.StreamIOIterWrapper(body), timeout=1)
        self.owner.register(reader)
        self.assertTrue(entered.wait(1))
        self.owner.request_stop(2)
        server.sendall(b"-accepted-current-read")
        self.assertEqual(self.drain(reader), b"prefix-accepted-current-read")
        self.assert_drained()

    def test_http_decoder_errors_are_not_mistaken_for_requested_transport_eof(self):
        response, _ = self.response(b"invalid gzip stream", encoding="gzip", chunked=True)
        reader = self.wrap.StreamIOIterWrapper(control.HTTPBody(response, self.owner.stopped))
        self.owner.request_stop(2)
        with self.assertRaises(OSError):
            self.drain(reader)
        self.assertNotIn(("drained", ""), self.messages.events)

    def test_http_premature_eof_before_stop_remains_an_error(self):
        response, server = self.response(b"partial", declared_length=1000)
        reader = self.wrap.StreamIOIterWrapper(control.HTTPBody(response, self.owner.stopped))
        server.close()
        with self.assertRaises(OSError):
            self.drain(reader)
        self.assertNotIn(("drained", ""), self.messages.events)

    def test_tls_decrypted_pending_tail_is_drained_without_another_network_read(self):
        with tempfile.TemporaryDirectory() as directory:
            cert, key = tls_certificate(directory)
            server_context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
            server_context.load_cert_chain(cert, key)
            client_context = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
            client_context.check_hostname = False
            client_context.verify_mode = ssl.CERT_NONE
            left, right = socket.socketpair()
            left.settimeout(2)
            right.settimeout(2)
            self.connections.extend([left, right])
            # Exercise sender backpressure even where the default socket buffer
            # can hold the whole application record before any client read.
            right.setsockopt(socket.SOL_SOCKET, socket.SO_SNDBUF, 1024)
            release, sent = threading.Event(), threading.Event()
            server_errors = []
            payload = b"tls-pending-tail" * 1000
            headers = {"Content-Length": str(len(payload)), "Connection": "keep-alive"}
            wire = b"HTTP/1.1 200 OK\r\n" + b"".join(
                f"{key}: {value}\r\n".encode() for key, value in headers.items()) + b"\r\n" + payload
            self.assertLessEqual(len(wire), 16384, "the accepted payload must fit in one TLS record")
            def serve():
                try:
                    with server_context.wrap_socket(right, server_side=True) as connection:
                        connection.sendall(wire)
                        sent.set()
                        release.wait(3)
                except Exception as error:
                    server_errors.append(error)
            server = threading.Thread(target=serve, daemon=True)
            server.start()
            client = response = None
            try:
                client = client_context.wrap_socket(left, server_hostname="localhost")
                self.connections.append(client)
                makefile = client.makefile
                # Begin reading while sendall can still be blocked on socket
                # capacity. Waiting for send completion first deadlocks on macOS.
                # Python 3.14's default 128 KiB buffer can consume the entire TLS
                # record while parsing headers. Use a legal smaller buffer so
                # this fixture proves the distinct SSLSocket.pending boundary.
                with mock.patch.object(client, "makefile", side_effect=lambda mode: makefile(mode, buffering=1024)):
                    response = self.parsed_response(client, headers)
                self.assertTrue(sent.wait(2), server_errors)
                self.assertGreater(client.pending(), 0, "fixture must stage a decrypted TLS tail")
                reader = self.wrap.StreamIOIterWrapper(control.HTTPBody(response, self.owner.stopped))
                self.owner.request_stop(2)
                self.assertEqual(self.drain(reader), payload)
                self.assert_drained()
            finally:
                release.set()
                if response is not None:
                    response.close()
                if client is not None:
                    client.close()
                left.close()
                right.close()
                server.join(2)
                self.assertFalse(server.is_alive(), "TLS fixture server must settle")
            self.assertFalse(server_errors)

    def assert_manifest_reload_stops(self, dash=False):
        if dash:
            from datetime import datetime, timezone
            module = self.modules["streamlink.stream.dash.dash"]
            stream = types.SimpleNamespace(session=self.session, args={}, mpd=types.SimpleNamespace(url="https://invalid.test/manifest"), duration=0)
            reader = module.DASHStreamReader(stream, types.SimpleNamespace(ident="video", mimeType="video/mp2t"), datetime.now(timezone.utc))
        else:
            hls = self.modules["streamlink.stream.hls.hls"]
            reader = hls.HLSStreamReader(hls.HLSStream(self.session, "https://invalid.test/live.m3u8"))
        reader.buffer.write(b"x" * 32)
        waiting = threading.Event()
        wait_free = reader.buffer.wait_free
        def wait():
            waiting.set()
            return wait_free()
        reader.buffer.wait_free = wait
        self.owner.register(reader)
        with mock.patch.object(self.session.http, "get") as fetch:
            thread = threading.Thread(target=reader.worker.reload, daemon=True)
            thread.start()
            self.assertTrue(waiting.wait(1))
            self.owner.request_stop(2)
            reader.buffer.read(32)
            thread.join(2)
            self.assertFalse(thread.is_alive())
            fetch.assert_not_called()
        reader.close()

    def test_hls_reload_does_not_acquire_after_a_full_ring_wait(self):
        self.assert_manifest_reload_stops()

    def test_dash_reload_does_not_acquire_after_a_full_ring_wait(self):
        self.assert_manifest_reload_stops(dash=True)

    def test_windows_pipe_checks_and_both_hidden_launch_paths_without_native_calls(self):
        import asyncio
        import ctypes
        class Function:
            def __init__(self, call):
                self.call = call
            def __call__(self, *args):
                return self.call(*args)
        events, data = [], bytearray()
        def write(handle, buffer, length, written, overlap):
            count = min(3, length)
            data.extend(ctypes.string_at(buffer, count))
            written._obj.value = count
            return 1
        kernel = types.SimpleNamespace(
            CreateNamedPipeW=Function(lambda *args: ctypes.c_void_p(-1).value),
            ConnectNamedPipe=Function(lambda *args: 1),
            WriteFile=Function(write),
            FlushFileBuffers=Function(lambda *args: events.append("flush") or 1),
            DisconnectNamedPipe=Function(lambda *args: events.append("disconnect") or 1),
            CloseHandle=Function(lambda *args: events.append("close") or 1),
            GetLastError=Function(lambda: 5),
        )
        dll = types.SimpleNamespace(kernel32=kernel)
        mux = self.modules["streamlink.stream.ffmpegmux"]
        pipes = self.modules["streamlink.utils.named_pipe"]
        output = self.modules["streamlink.utils.processoutput"]
        mux_launch = mock.Mock(return_value="muxer")
        validation_launch = mock.AsyncMock(return_value="validated")
        restored = []
        def patch(target, name, value):
            restored.append((target, name, inspect.getattr_static(target, name)))
            setattr(target, name, value)
        with mock.patch.object(ctypes, "windll", dll, create=True), mock.patch.object(pipes, "windll", dll, create=True), \
                mock.patch.object(mux, "subprocess", types.SimpleNamespace(Popen=mux_launch)), \
                mock.patch.object(output, "trio", types.SimpleNamespace(run_process=validation_launch)):
            try:
                control.install_windows(self.modules, patch)
                with self.assertRaises(OSError):
                    pipes.NamedPipeWindows()
                kernel.CreateNamedPipeW.call = lambda *args: 123
                pipe = pipes.NamedPipeWindows()
                pipe.open()
                self.assertEqual(pipe.write(b"checked tail"), len(b"checked tail"))
                pipe.close()
                self.assertEqual(data, b"checked tail")
                self.assertEqual(events, ["flush", "disconnect", "close"])
                self.assertEqual(mux.subprocess.Popen(["ffmpeg"], creationflags=0x200), "muxer")
                self.assertEqual(asyncio.run(output.trio.run_process(["ffmpeg", "-version"], creationflags=0x400)), "validated")
                self.assertEqual(mux_launch.call_args.kwargs["creationflags"], 0x08000200)
                self.assertEqual(validation_launch.call_args.kwargs["creationflags"], 0x08000400)
            finally:
                for target, name, value in reversed(restored):
                    setattr(target, name, value)

    def test_filtered_stop_waits_for_an_admitted_fetch_after_the_old_read_timeout(self):
        hls = self.modules["streamlink.stream.hls.hls"]
        reader = hls.HLSStreamReader(hls.HLSStream(self.session, "https://invalid.test/live.m3u8"))
        entered, release = threading.Event(), threading.Event()
        payload = b"accepted late fetch" * 7
        def fetch(segment):
            entered.set()
            self.assertTrue(release.wait(2))
            return payload
        reader.writer.fetch = fetch
        reader.writer.write = lambda segment, data, *extra: reader.buffer.write(data)
        def segments():
            yield types.SimpleNamespace(num=0, duration=1, uri="segment", map=None)
            reader.worker.wait(5)
        reader.worker.iter_segments = segments
        self.owner.opening = True
        reader.open()
        self.owner.opening = False
        reader.pause()
        self.assertTrue(entered.wait(1))
        result = []
        consumer = threading.Thread(target=lambda: result.append(self.drain(reader)), daemon=True)
        consumer.start()
        time.sleep(0.15)
        self.owner.request_stop(2)
        self.assertTrue(consumer.is_alive(), "the stale read timeout must not close accepted work")
        release.set()
        consumer.join(2)
        self.assertFalse(consumer.is_alive())
        self.assertEqual(result, [payload])
        self.assert_drained()

    def websocket_reader(self):
        return self.websocket.TwitCastingReader(
            types.SimpleNamespace(session=self.session, url="wss://invalid.test/"))

    def test_websocket_received_before_callback_is_not_dropped_by_stop(self):
        import websocket
        reader = self.websocket_reader()
        core = websocket.WebSocket()
        left, right = socket.socketpair()
        self.connections.extend([left, right])
        core.sock = left
        payload = b"received frame"
        core.recv_data_frame = lambda *args: (2, websocket.ABNF.create_frame(payload, 2))
        reader.wsclient._srec_acquisition.attach(core)
        operation, frame = core.recv_data_frame(True)
        self.owner.register(reader)
        self.owner.request_stop(2)
        self.assertFalse(reader.buffer.closed)
        reader.wsclient.on_data(None, frame.data, operation, True)
        self.assertEqual(self.drain(reader), payload)
        self.assert_drained()

    def test_websocket_stop_finishes_current_receive_before_closing(self):
        import websocket
        reader = self.websocket_reader()
        core = websocket.WebSocket()
        left, right = socket.socketpair()
        self.connections.extend([left, right])
        core.sock = left
        entered, release = threading.Event(), threading.Event()
        payload = b"inflight websocket frame"
        def receive(*args):
            entered.set()
            self.assertTrue(release.wait(2))
            return 2, websocket.ABNF.create_frame(payload, 2)
        core.recv_data_frame = receive
        reader.wsclient._srec_acquisition.attach(core)
        def consume():
            operation, frame = core.recv_data_frame(True)
            reader.wsclient.on_data(None, frame.data, operation, True)
        thread = threading.Thread(target=consume, daemon=True)
        reader.wsclient.is_alive = thread.is_alive
        reader.wsclient.join = thread.join
        self.owner.register(reader)
        thread.start()
        self.assertTrue(entered.wait(1))
        with mock.patch.object(reader.wsclient.ws, "close", wraps=reader.wsclient.ws.close) as close:
            self.owner.request_stop(2)
            close.assert_not_called()
            release.set()
            self.assertEqual(self.drain(reader), payload)
        thread.join(2)
        self.assert_drained()

    def test_websocket_stop_before_open_never_starts_a_connection(self):
        reader = self.websocket_reader()
        self.owner.request_stop(2)
        with mock.patch.object(reader.wsclient, "start") as start:
            reader.open()
            start.assert_not_called()
        self.assertEqual(self.drain(reader), b"")
        self.assert_drained()

    def test_websocket_startup_cannot_reset_away_an_accepted_stop(self):
        reader = self.websocket_reader()
        entered, reset_allowed, reset_done, closed_once = (threading.Event() for _ in range(4))
        def run_forever(**kwargs):
            entered.set()
            self.assertTrue(reset_allowed.wait(2))
            reader.wsclient.ws.keep_running = True
            reset_done.set()
            deadline = time.monotonic() + 2
            while reader.wsclient.ws.keep_running:
                if time.monotonic() >= deadline:
                    self.fail("startup lost the stop latch")
                time.sleep(0.001)
        def close(**kwargs):
            reader.wsclient.ws.keep_running = False
            closed_once.set()
        reader.wsclient.ws.run_forever = run_forever
        reader.wsclient.ws.close = close
        reader.open()
        self.assertTrue(entered.wait(1))
        self.owner.request_stop(2)
        self.assertTrue(closed_once.wait(1))
        reset_allowed.set()
        self.assertTrue(reset_done.wait(1))
        reader.wsclient.join(2)
        self.assertFalse(reader.wsclient.is_alive())
        self.assertEqual(self.drain(reader), b"")
        self.assert_drained()


class PureContracts(unittest.TestCase):
    def test_source_profile_must_match_actual_sources(self):
        self.assertEqual(len(control.audited_modules()), len(control.SOURCES))
        with mock.patch.dict(control.SOURCES, {"streamlink.buffers": "0" * 64}):
            with self.assertRaises(ValueError):
                control.audited_modules()

    def test_checked_pipe_write_and_flush_order(self):
        accepted = bytearray()
        events = []
        def partial(data):
            accepted.extend(data[:3])
            return len(data[:3])
        self.assertEqual(control.write_all(b"complete tail", partial), 13)
        control.flush_before_disconnect(lambda: events.append("read by client"),
                                        lambda: events.append("disconnect"))
        self.assertEqual(accepted, b"complete tail")
        self.assertEqual(events, ["read by client", "disconnect"])
        with self.assertRaises(OSError):
            control.write_all(b"lost", lambda data: 0)
        with self.assertRaises(OSError):
            control.flush_before_disconnect(lambda: (_ for _ in ()).throw(OSError()),
                                            lambda: self.fail("must not disconnect before flush"))

    def test_authenticated_stop_bounds_framing_and_deadlines(self):
        for message in [
            {"protocol": 1, "command": "stop", "token": "a" * 64, "budget_ms": 50},
            {"protocol": 1, "command": "stop", "token": "wrong", "budget_ms": 50},
            {"protocol": 1, "command": "stop", "token": "a" * 64, "budget_ms": -1},
            [],
        ]:
            left, right = socket.socketpair()
            with left, right:
                right.sendall(json.dumps(message).encode() + b"\n")
                protocol = control.Protocol(left, "a" * 64)
                if isinstance(message, dict) and message.get("token") == "a" * 64 and message["budget_ms"] == 50:
                    self.assertEqual(protocol.receive_stop(), 0.05)
                else:
                    with self.assertRaises(ValueError):
                        protocol.receive_stop()


if __name__ == "__main__":
    unittest.main()
