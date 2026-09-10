"""Offline Streamlink plugin copied alone into a temporary plugin directory."""
import json
from pathlib import Path
import re
from types import SimpleNamespace

from streamlink.plugin import Plugin, pluginargument, pluginmatcher
from streamlink.stream import HTTPStream
from streamlink.stream.ffmpegmux import MuxedStream
from streamlink.stream.file import FileStream
from streamlink.stream.segmented.segmented import SegmentedStreamReader
from streamlink.stream.stream import Stream


class AcceptedStream(Stream):
    def __init__(self, session, path, directory, natural):
        super().__init__(session)
        self.path = path
        self.directory = directory
        self.natural = natural

    def open(self):
        reader = SegmentedStreamReader(self)
        payload = self.path.read_bytes()
        cut = max(188, len(payload) // 4 // 188 * 188)
        chunks = [payload[index * cut:(index + 1) * cut] for index in range(3)] + [payload[3 * cut:]]
        def fetch(segment):
            if segment.num == 3 and not self.natural:
                # The final admitted job must still be pending when stop arrives.
                # Its bytes may only reach either muxer after producer admission
                # has closed; packet equality then proves an actual stop drain.
                (self.directory / (self.path.name + ".tail-held")).write_text("accepted fetch pending")
                reader.worker.wait(30)
                if not reader.worker.closed:
                    raise RuntimeError("stop never released the accepted tail")
                (self.directory / (self.path.name + ".tail-released")).write_text("producer stopped")
            return chunks[segment.num]
        reader.writer.fetch = fetch
        reader.writer.write = lambda segment, data: reader.buffer.write(data)
        def segments():
            # Both comparison runs admit exactly these jobs. No expected tail
            # depends on a future segment that stop was supposed to exclude.
            for index in range(4):
                yield SimpleNamespace(num=index, duration=1)
            (self.directory / (self.path.name + ".accepted")).write_text("four jobs")
            if not self.natural:
                reader.worker.wait(60)
        reader.worker.iter_segments = segments
        reader.open()
        return reader


class TrackedMux(MuxedStream):
    def __init__(self, *args, directory, label, **kwargs):
        super().__init__(*args, **kwargs)
        self.directory = directory
        self.label = label

    def open(self):
        fd = super().open()
        (self.directory / (self.label + ".pid")).write_text(str(fd.process.pid))
        return fd


@pluginmatcher(re.compile(r"^srec-fixture://(?P<mode>nested|http)$"))
@pluginargument("directory", required=True)
@pluginargument("marker", default="kept")
@pluginargument("natural", action="store_true")
class Fixture(Plugin):
    def _get_streams(self):
        directory = Path(self.options.get("directory"))
        (directory / "options.json").write_text(json.dumps({"marker": self.options.get("marker")}))
        if self.match["mode"] == "http":
            yield "source", HTTPStream(self.session, (directory / "url").read_text())
            return
        natural = bool(self.options.get("natural"))
        video = AcceptedStream(self.session, directory / "video.ts", directory, natural)
        audio = AcceptedStream(self.session, directory / "audio.ts", directory, natural)
        inner = TrackedMux(self.session, video, audio, directory=directory, label="inner")
        # A sparse subtitle after the A/V endpoint tests the last input's tail
        # through a second, genuinely nested muxer.
        subtitle = FileStream(self.session, fileobj=__import__("io").BytesIO(
            b"1\n00:00:00,000 --> 00:00:00,200\nfirst\n\n"
            b"2\n00:00:04,000 --> 00:00:05,000\nlast subtitle tail\n\n"))
        yield "source", TrackedMux(self.session, inner, subtitles={"eng": subtitle},
                                   maps=["0:v", "0:a", "1:0"],
                                   directory=directory, label="outer")


__plugin__ = Fixture
