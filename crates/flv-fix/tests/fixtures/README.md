These are single AC-3 and E-AC-3 frames containing stereo silence at 48 kHz and
128 kbit/s. They exercise codec changes with actual media data; tests do not
require FFmpeg.

Regenerate with FFmpeg:

```sh
ffmpeg -f lavfi -i anullsrc=r=48000:cl=stereo -frames:a 1 -c:a ac3 -b:a 128k -f ac3 ac3-silence.frame
ffmpeg -f lavfi -i anullsrc=r=48000:cl=stereo -frames:a 1 -c:a eac3 -b:a 128k -f eac3 eac3-silence.frame
```

Some FFmpeg versions emit an extra frame while flushing. Keep only the first
512-byte frame in each file:

```sh
python -c "from pathlib import Path; [p.write_bytes(p.read_bytes()[:512]) for p in map(Path, ('ac3-silence.frame', 'eac3-silence.frame'))]"
```
