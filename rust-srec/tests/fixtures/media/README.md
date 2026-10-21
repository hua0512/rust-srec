# Media fixtures

Media the end-to-end fake platform serves (`tests/e2e/support/media.rs`).
Made with ffmpeg n9.0.2; regenerate from this directory with the command
below. Other ffmpeg builds produce slightly different bytes, which the tests
do not depend on.

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
