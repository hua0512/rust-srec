# Initialization segment fixtures

Single-track initialization segments produced by FFmpeg's DASH muxer, used to
check codec detection, codec configuration and resolution extraction against
encoder output. Each video codec uses a different frame size so a test cannot
pass by accident. Generate each command in its own temporary directory and
copy the named `init-streamN.m4s`:

```sh
# h264-112x64.mp4 (init-stream0.m4s) and aac-mono.mp4 (init-stream1.m4s)
ffmpeg -f lavfi -i testsrc2=size=112x64:rate=24 -f lavfi -i sine=frequency=440:sample_rate=48000 -t 1 -c:v libx264 -preset veryfast -c:a aac -ac 1 -b:a 24k -f dash -seg_duration 1 -adaptation_sets "id=0,streams=v id=1,streams=a" out.mpd
# hevc-96x48.mp4 (init-stream0.m4s)
ffmpeg -f lavfi -i testsrc2=size=96x48:rate=24 -t 1 -c:v libx265 -preset ultrafast -tag:v hvc1 -f dash -seg_duration 1 out.mpd
# av1-80x64.mp4 (init-stream0.m4s)
ffmpeg -f lavfi -i testsrc2=size=80x64:rate=24 -t 1 -c:v libsvtav1 -preset 12 -f dash -dash_segment_type mp4 -seg_duration 1 out.mpd
```
