# TS regression fixtures

These synthetic black frames were generated with FFmpeg/libx264. They contain
real SPS/PPS, separate PAT/PMT and video PIDs, valid PSI CRCs, and a random-access
indicator. Tests load the checked-in bytes and require no FFmpeg installation.

From this directory, generate each size (640x352 and 1280x720):

```sh
ffmpeg -f lavfi -i color=c=black:size=640x352:rate=25 -frames:v 1 -c:v libx264 -preset ultrafast -g 1 -f mpegts avc-640x352.ts
```

Verify with `ffprobe -v error -count_frames -show_streams avc-640x352.ts`.
Each fixture contains one H.264 frame at the dimensions in its filename.
The HLS splitter tests also use these fixtures.
