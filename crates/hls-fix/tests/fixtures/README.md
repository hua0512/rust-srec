# fMP4 regression fixtures

These synthetic black frames were generated with FFmpeg/libx264. Tests use the
checked-in initialization sections and media fragments, so FFmpeg is not a test
dependency. Each fragment contains one independently decodable H.264 frame.

From this directory, generate the 32x32 sequence:

```sh
ffmpeg -f lavfi -i color=c=black:size=32x32:rate=1 -t 2 -c:v libx264 -preset ultrafast -g 1 -f hls -hls_time 1 -hls_list_size 0 -hls_segment_type fmp4 -hls_fmp4_init_filename init.mp4 -hls_segment_filename media%02d.m4s stream.m3u8
```

For the configuration-change fixtures, run the same command in a temporary
directory with `size=64x64` and `-t 1`. Copy `init.mp4` and `media00.m4s` to
`init-64x64.mp4` and `media-64x64.m4s` respectively.

Verify the playlist with `ffprobe -v error -count_frames -show_streams stream.m3u8`.
For a standalone file, concatenate an init with its matching fragment(s) in
binary mode; the 32x32 sequence decodes to two frames, and the 64x64 sequence to
one. Fixtures may differ across FFmpeg versions; regenerate and verify them
together rather than relying on byte-for-byte encoder reproducibility.
