"""Local media/CLI subprocess tests. Run separately under serialized native validation.

SREC_STREAMLINK_NATIVE=1 is mandatory: the pure companion suite never spawns
Streamlink, FFmpeg or rustc. No fixture contacts a live streaming service.
"""
import collections
import json
import os
from pathlib import Path
import secrets
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import unittest

from process_tree import NativeProcess, run as run_native

HERE = Path(__file__).resolve().parent
SOURCE = HERE.parents[1] / "src/downloader/engine/streamlink/srec_control.py"
ENABLED = os.environ.get("SREC_STREAMLINK_NATIVE") == "1"


def process_options():
    return {"creationflags": 0x08000000} if os.name == "nt" else {"start_new_session": True}


def bounded(command, **kwargs):
    return run_native(command, **kwargs)


def contain(process):
    process.close()


class Peer:
    def __init__(self):
        self.listener = socket.socket()
        self.listener.bind(("127.0.0.1", 0))
        self.listener.listen(1)
        self.listener.settimeout(10)
        self.token = secrets.token_hex(32)
        self.connection = None

    def environment(self):
        return dict(os.environ, SREC_STREAMLINK_CONTROL_TOKEN=self.token,
                    SREC_STREAMLINK_CONTROL_PORT=str(self.listener.getsockname()[1]))

    def receive(self):
        deadline = time.monotonic() + 10
        if self.connection is None:
            self.connection, _ = self.listener.accept()
        data = bytearray()
        while len(data) < 4096:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError("companion frame deadline expired")
            self.connection.settimeout(remaining)
            byte = self.connection.recv(1)
            if not byte:
                raise AssertionError("companion closed without completion")
            if byte == b"\n":
                message = json.loads(data)
                if (message["token"] != self.token or message["protocol"] != 1
                        or message["profile"] != "streamlink-8.5.0-drain-v1"):
                    raise AssertionError("invalid companion identity")
                return message
            data.extend(byte)
        raise AssertionError("oversized companion frame")

    def stop(self, budget=8000):
        self.connection.sendall(json.dumps({"token": self.token, "protocol": 1,
                                           "command": "stop", "budget_ms": budget}).encode() + b"\n")

    def close(self):
        if self.connection:
            self.connection.close()
        self.listener.close()


@unittest.skipUnless(ENABLED, "separate native validation; set SREC_STREAMLINK_NATIVE=1")
class NativeControlTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.ffmpeg = shutil.which("ffmpeg")
        cls.ffprobe = shutil.which("ffprobe")
        cls.rustc = shutil.which("rustc")
        cli = Path(sys.executable).with_name("streamlink.exe" if os.name == "nt" else "streamlink")
        cls.cli = str(cli if cli.exists() else shutil.which("streamlink") or "")
        if not all((cls.ffmpeg, cls.ffprobe, cls.rustc, cls.cli)):
            raise RuntimeError("native tests require the pinned Streamlink environment, ffmpeg, ffprobe and rustc")

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="srec-native-control-")
        self.addCleanup(self.temp.cleanup)
        self.directory = Path(self.temp.name)
        self.plugin = self.directory / "plugins"
        self.control = self.directory / "control"
        self.plugin.mkdir()
        self.control.mkdir()
        shutil.copyfile(HERE / "fixture_plugin.py", self.plugin / "fixture.py")
        shutil.copyfile(SOURCE, self.control / "srec_control.py")
        self.ffmpeg_shim = self.directory / ("ffmpeg_shim.exe" if os.name == "nt" else "ffmpeg_shim")
        bounded([self.rustc, "--edition=2021", str(HERE / "ffmpeg_console.rs"), "-o", str(self.ffmpeg_shim)])

    def media(self, directory):
        directory.mkdir()
        bounded([self.ffmpeg, "-v", "error", "-f", "lavfi", "-i", "testsrc=size=64x64:rate=5",
                 "-t", "4", "-threads", "1", "-filter_threads", "1", "-c:v", "mpeg2video", "-f", "mpegts", str(directory / "video.ts")])
        bounded([self.ffmpeg, "-v", "error", "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000",
                 "-t", "4", "-threads", "1", "-c:a", "mp2", "-f", "mpegts", str(directory / "audio.ts")])

    def record(self, directory, natural):
        peer = Peer()
        result = directory / "result.mkv"
        log = directory / "stderr.log"
        command = [self.cli, "--no-config", "--no-plugin-sideloading", "--stdout", "--loglevel", "error", "--plugin-dir", str(self.control),
                   "--plugin-dir", str(self.plugin), "--fixture-directory", str(directory),
                   "--fixture-marker", "custom-option-preserved", "--ffmpeg-ffmpeg", str(self.ffmpeg_shim),
                   "--ringbuffer-size", "32k", "--stream-timeout", "10"]
        if natural:
            command.append("--fixture-natural")
        command += ["srec-fixture://nested", "source"]
        with result.open("wb") as output, log.open("wb") as errors:
            environment = peer.environment()
            environment.update(SREC_NATIVE_REAL_FFMPEG=self.ffmpeg,
                               SREC_NATIVE_CONSOLE_LOG=str(directory / "console.log"))
            process = NativeProcess(command, stdout=output, stderr=errors,
                                    env=environment, **process_options())
            try:
                self.assertEqual(peer.receive()["event"], "ready")
                if not natural:
                    deadline = time.monotonic() + 10
                    while (not all((directory / (name + suffix)).exists()
                                   for name in ("video.ts", "audio.ts")
                                   for suffix in (".accepted", ".tail-held"))
                           or len(list(directory.glob("*.pid"))) < 2):
                        self.assertIsNone(process.poll(), log.read_text(errors="replace"))
                        self.assertLess(time.monotonic(), deadline, "accepted-input barrier did not arrive")
                        time.sleep(0.01)
                    self.assertFalse(any(directory.glob("*.tail-released")))
                    peer.stop()
                    message = peer.receive()
                    self.assertEqual(message["event"], "drained", message.get("reason"))
                    self.assertTrue(all((directory / (name + ".tail-released")).exists()
                                        for name in ("video.ts", "audio.ts")))
                self.assertEqual(process.wait(timeout=12), 0, log.read_text(errors="replace"))
                if os.name == "nt":
                    self.assert_hidden(directory)
            finally:
                contain(process)
                peer.close()
        self.assertEqual(json.loads((directory / "options.json").read_text())["marker"],
                         "custom-option-preserved")
        return result

    def packets(self, path):
        result = bounded([self.ffprobe, "-v", "error", "-show_packets", "-show_data_hash", "sha256",
                          "-show_entries", "packet=stream_index,pts_time,dts_time,duration_time,size,data_hash",
                          "-of", "json", str(path)])
        tracks = collections.defaultdict(list)
        for packet in json.loads(result.stdout)["packets"]:
            tracks[packet.pop("stream_index")].append(packet)
        return dict(tracks)

    def assert_hidden(self, directory):
        observed = [line.split(":") for line in (directory / "console.log").read_text().splitlines()]
        self.assertTrue(any(role == "validation" for role, _ in observed), "the validation child must be observed")
        self.assertGreaterEqual(sum(role == "muxer" for role, _ in observed), 2)
        self.assertTrue(all(window == "0" for _, window in observed), observed)

    def test_nested_muxed_video_audio_and_sparse_subtitle_match_natural_eof(self):
        reference, stopped = self.directory / "reference", self.directory / "stopped"
        self.media(reference)
        audio_packets = sum(len(packets) for packets in self.packets(reference / "audio.ts").values())
        shutil.copytree(reference, stopped)
        expected = self.packets(self.record(reference, True))
        actual = self.packets(self.record(stopped, False))
        self.assertEqual(len(actual), 3, "video, audio and sparse subtitle must survive")
        self.assertEqual(actual, expected)
        self.assertEqual(len(actual[0]), 20, "all four seconds of five-fps video must survive")
        self.assertEqual(len(actual[1]), audio_packets, "every admitted audio packet must survive both muxers")
        self.assertEqual(len(actual[2]), 2, "both sparse subtitles must survive")
        self.assertGreaterEqual(float(actual[2][-1]["pts_time"]), 4,
                        "the sparse final subtitle must survive the nested pipe drain")

    @unittest.skipUnless(os.name == "nt", "Windows console negative control")
    def test_console_observer_detects_an_explicit_hidden_console_negative_control(self):
        startup = subprocess.STARTUPINFO()
        startup.dwFlags |= subprocess.STARTF_USESHOWWINDOW
        startup.wShowWindow = 0  # SW_HIDE: negative control must never display a window
        log = self.directory / "negative-console.log"
        environment = dict(os.environ, SREC_NATIVE_REAL_FFMPEG=self.ffmpeg, SREC_NATIVE_CONSOLE_LOG=str(log))
        bounded([str(self.ffmpeg_shim), "-version"], env=environment,
                creationflags=0x10, startupinfo=startup)  # CREATE_NEW_CONSOLE
        role, window = log.read_text().strip().split(":")
        self.assertEqual(role, "validation")
        self.assertNotEqual(window, "0", "observer must distinguish an existing hidden console from CREATE_NO_WINDOW")

    def test_actual_help_probe_loads_and_authenticates_the_companion(self):
        peer = Peer()
        process = NativeProcess([self.cli, "--no-config", "--no-plugin-sideloading", "--plugin-dir", str(self.control), "--help"],
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                                   env=peer.environment(), **process_options())
        try:
            self.assertEqual(peer.receive()["event"], "ready")
            self.assertEqual(process.wait(timeout=5), 0)
        finally:
            contain(process)
            peer.close()

    def tree_fixture(self):
        executable = self.directory / ("tree_fixture.exe" if os.name == "nt" else "tree_fixture")
        bounded([self.rustc, "--edition=2021", str(HERE / "process_tree_fixture.rs"), "-o", str(executable)])
        return executable

    def test_owned_launcher_settles_descendants_after_the_leader_exits(self):
        ready = self.directory / "descendant-ready"
        with NativeProcess([str(self.tree_fixture()), "exit", str(ready)],
                           stdout=subprocess.PIPE, stderr=subprocess.PIPE) as process:
            output, errors = process.communicate(timeout=5)
            self.assertEqual(process.returncode, 0, errors)
            self.assertTrue(ready.exists(), "the descendant must be admitted before leader exit")
            self.assertEqual(output, b"leaf owns stdout\n", "inherited stdout must reach EOF after tree cleanup")

    def test_owned_launcher_timeout_settles_the_whole_tree_and_its_pipe(self):
        ready = self.directory / "descendant-ready"
        with NativeProcess([str(self.tree_fixture()), "hold", str(ready)],
                           stdout=subprocess.PIPE, stderr=subprocess.PIPE) as process:
            deadline = time.monotonic() + 5
            while not ready.exists():
                self.assertIsNone(process.poll())
                self.assertLess(time.monotonic(), deadline, "descendant startup timed out")
                time.sleep(0.005)
            started = time.monotonic()
            with self.assertRaises(subprocess.TimeoutExpired):
                process.communicate(timeout=0.1)
            process.close(timeout=5)
            self.assertLess(time.monotonic() - started, 5.5)
            self.assertIsNotNone(process.poll())
            self.assertEqual(process.captured("stdout"), b"leaf owns stdout\n")

    def test_owned_launcher_output_limit_contains_the_tree(self):
        from process_tree import OutputLimitExceeded
        ready = self.directory / "descendant-ready"
        with NativeProcess([str(self.tree_fixture()), "flood", str(ready)],
                           stdout=subprocess.PIPE, stderr=subprocess.PIPE, output_limit=1024) as process:
            with self.assertRaises(OutputLimitExceeded):
                process.communicate(timeout=5)
            process.close(timeout=5)
            self.assertTrue(ready.exists())
            self.assertLessEqual(len(process.captured("stdout")), 1024)

    def test_same_version_opaque_clis_can_reject_or_ignore_control_options_and_still_record(self):
        reject = self.directory / ("opaque_reject.exe" if os.name == "nt" else "opaque_reject")
        ignore = self.directory / ("opaque_ignore.exe" if os.name == "nt" else "opaque_ignore")
        bounded([self.rustc, "--edition=2021", str(HERE / "opaque_cli.rs"), "-o", str(reject)])
        shutil.copy2(reject, ignore)
        for executable in (reject, ignore):
            self.assertIn(b"8.5.0", bounded([str(executable), "--version"]).stdout)
            probe = bounded([str(executable), "--plugin-dir", str(self.control), "--help"], timeout=5, check=False)
            self.assertEqual(probe.returncode, 2 if executable == reject else 0)
            self.assertEqual(bounded([str(executable), "--stdout", "srec-fixture://opaque", "best"]).stdout,
                             b"opaque recording remains available\n")


if __name__ == "__main__":
    unittest.main()
