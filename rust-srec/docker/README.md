# Backend runtime image inputs

`runtime-tools.json` is the reviewable lock for downloaded executables and the
Twitch plugin. The installer downloads the exact versioned URL, checks the
checked-in SHA256 before opening the archive, and copies only the named members.
It does not execute downloaded installation scripts. Both Linux architectures use
the same versions:

| Component | Version | Upstream provenance |
| --- | --- | --- |
| FFmpeg, ffprobe, ffplay | N-126435-gf93cd72dde | [yt-dlp autobuild 2026-09-06](https://github.com/yt-dlp/FFmpeg-Builds/releases/tag/autobuild-2026-09-06-16-53) |
| rclone | 1.75.1 | [rclone release](https://github.com/rclone/rclone/releases/tag/v1.75.1) |
| DanmakuFactory | 2.0.0 | [DanmakuFactory release](https://github.com/renmu123/DanmakuFactory/releases/tag/v2.0.0) |
| BaiduPCS-Go | 4.0.1 | [BaiduPCS-Go release](https://github.com/qjfoidnh/BaiduPCS-Go/releases/tag/v4.0.1) |
| Streamlink | 8.5.0 | [PyPI distribution](https://pypi.org/project/streamlink/8.5.0/) |
| Streamlink ttvlol | 8.3.0-20260701 | [ttvlol release](https://github.com/2bc4/streamlink-ttvlol/releases/tag/8.3.0-20260701) |
| cargo-chef (builder only) | 0.1.78 | [crate release](https://crates.io/crates/cargo-chef/0.1.78) |

The executable/plugin checksums were checked against GitHub's release-asset
`digest` metadata and the downloaded bytes. `streamlink-requirements.txt` pins
the complete Linux CPython 3.11 dependency graph with wheel hashes verified
against each project's PyPI JSON metadata. `pip --require-hashes` enforces the
lock in the image; source distributions and unpinned Python dependency versions
are not permitted. amd64/arm64 native-wheel hashes are both included.

To update, select upstream releases, review their changes, fetch the versioned
assets and compare hashes with upstream metadata, then update the manifest.
Resolve Streamlink's dependencies for Linux CPython 3.11 on both architectures
and refresh both sets of hashes together. A mismatched artifact must fail the
build; do not replace a checksum just to bypass the failure. The Debian Bookworm
base tag and Debian security packages remain updateable, so this is a lock on
downloaded tools, not a claim that the entire image is bit-for-bit reproducible.

Streamlink's wrapper loads the immutable bundled plugin first and then the
writable `${XDG_DATA_HOME}/streamlink/plugins` directory. This follows the
[upstream sideloading interface](https://streamlink.github.io/cli/plugin-sideloading.html),
without modifying Streamlink's cached built-in plugin metadata. A deliberately
mounted custom `twitch.py` can override the bundled plugin.

The image defaults to UID/GID `1000:1000`. Compose's `PUID`/`PGID` are numeric
`user:` substitutions; direct Docker users use `--user UID:GID`. No root startup
or recursive ownership repair runs inside the image. Prepare mounted data,
configuration, output and log directories for the chosen identity. Tool state is
under `/app/config`, including Streamlink plugins/cache and BaiduPCS-Go settings.

The `Backend Runtime Image` pull-request workflow builds the real Dockerfile on
native amd64 and arm64 runners, without publishing. Its smoke test requires
healthy startup with authentication enabled, every bundled executable and the
Twitch plugin, writable default directories, a custom numeric UID/GID with bind
mounts, an overridden API port, and graceful exit. A source-only or download-only
check does not replace this native build/smoke gate.
