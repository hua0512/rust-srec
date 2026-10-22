# Media fixtures

Media the end-to-end fake platform and fake CDN serve
(`tests/e2e/support/media.rs` and `tests/e2e/support/cdn.rs`). Made with
ffmpeg n9.0.2; regenerate from this directory with the commands below.
Other ffmpeg builds produce slightly different bytes, which the tests do not
depend on.

## `tiny.flv`

Two seconds of 32x32 H.264 at 1 fps, every frame a keyframe, with 16 kbps
mono AAC; about 7 KB.

```sh
ffmpeg -y \
  -f lavfi -i "testsrc2=size=32x32:rate=1:duration=2" \
  -f lavfi -i "sine=frequency=440:sample_rate=16000:duration=2" \
  -map 0:v -map 1:a \
  -c:v libx264 -preset veryslow -profile:v baseline -pix_fmt yuv420p -g 1 -bf 0 \
  -c:a aac -b:a 16k -ac 1 \
  -map_metadata -1 -fflags +bitexact -flags +bitexact \
  -f flv tiny.flv
```

## `tiny0.ts`, `tiny1.ts`

The same two seconds as `tiny.flv`, as two one-second MPEG-TS segments of an
HLS stream with continuous timestamps; about 8 KB together. ffmpeg also
writes a playlist, which the tests do not use; delete it.

```sh
ffmpeg -y \
  -f lavfi -i "testsrc2=size=32x32:rate=1:duration=2" \
  -f lavfi -i "sine=frequency=440:sample_rate=16000:duration=2" \
  -map 0:v -map 1:a \
  -c:v libx264 -preset veryslow -profile:v baseline -pix_fmt yuv420p -g 1 -bf 0 \
  -c:a aac -b:a 16k -ac 1 \
  -map_metadata -1 -fflags +bitexact -flags +bitexact \
  -f hls -hls_time 1 -hls_list_size 0 -hls_segment_type mpegts \
  -hls_segment_filename 'tiny%d.ts' tiny.m3u8
```

The fMP4 and DASH streams reuse the fixtures of `crates/hls-fix` and
`crates/mp4`.
