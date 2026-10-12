# Separate-track DASH fixtures

Synthetic test pattern and tone generated with FFmpeg's DASH muxer: one
initialization segment and two 1-second media segments for a 64x64 H.264
(High profile, with B-frames) video track, and the same for a mono AAC audio
track (whose last segment holds the remaining tail). Both sources use track
ID 1, different movie timescales (1000 and 48000) and edit lists, and their
media segments start with `styp` and `sidx` — the layout the merger rewrites.
Tests use the checked-in files, so FFmpeg is not a test dependency.

Generate them in a temporary directory, then copy `init-stream0.m4s` to
`video-init.mp4`, `chunk-stream0-0000N.m4s` to `video-N.m4s`, and the
`stream1` files to `audio-init.mp4` / `audio-N.m4s`:

```sh
ffmpeg -f lavfi -i testsrc2=size=64x64:rate=24 -f lavfi -i sine=frequency=440:sample_rate=48000 -t 2 -c:v libx264 -preset veryfast -profile:v high -g 24 -keyint_min 24 -sc_threshold 0 -b:v 40k -c:a aac -ac 1 -b:a 24k -f dash -seg_duration 1 -use_template 1 -use_timeline 0 -adaptation_sets "id=0,streams=v id=1,streams=a" manifest.mpd
```

To check a merged result by hand, write it with the `merge_fmp4` example and
decode it: `ffmpeg -v error -i merged.mp4 -f null -`. Fixtures may differ
across FFmpeg versions; regenerate them together rather than relying on
byte-for-byte encoder reproducibility.
