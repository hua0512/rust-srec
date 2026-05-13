<p align="center">
  <img src="rust-srec/docs/public/stream-rec-orange.svg" width="120" alt="rust-srec logo" />
</p>

<h1 align="center">rust-srec</h1>

<p align="center">
  <b>Automated live-stream recorder for the platforms you actually watch.</b><br/>
  Set it up once, and it captures your favorite streamers the moment they go live.
</p>

<p align="center">
  <a href="./README.zh-CN.md">简体中文</a> ·
  <a href="https://docs.srec.rs">Documentation</a> ·
  <a href="https://docs.srec.rs/en/getting-started/">Get Started</a> ·
  <a href="https://docs.srec.rs/en/getting-started/docker">Docker</a> ·
  <a href="https://docs.srec.rs/en/release-notes/">Release Notes</a>
</p>

---

## What it does

- **Records streams automatically.** Add a streamer, walk away. rust-srec watches and records when they go live.
- **Captures chat too.** Danmaku and live chat are saved alongside the video where the platform allows.
- **Fixes broken recordings.** Built-in repair for FLV and HLS streams handles timestamp drift, missing metadata, and similar issues.
- **Stays out of your way.** Lightweight, runs in Docker, and offers a web UI plus a REST API with Swagger docs.

## Supported platforms

Bilibili · Douyin · Douyu · Huya · Twitch · TikTok · AcFun · Picarto · Redbook · TwitCasting · Weibo · PandaTV (legacy)

## Quick start (Docker)

The fastest way to try it:

```bash
# grab the compose file and edit VERSION / volumes as needed
curl -O https://raw.githubusercontent.com/hua0512/rust-srec/main/rust-srec/docker-compose.yml
docker compose up -d
```

Then open the web UI and follow the [getting-started guide](https://docs.srec.rs/en/getting-started/).

> Image tags use a leading `v` — set `VERSION=v0.3.1`, not `0.3.1`.

Prefer a binary or to build from source? See the [installation guide](https://docs.srec.rs/en/getting-started/installation).

## Highlights

- **Multi-platform** — 12 streaming sites supported out of the box.
- **Three download engines** — pick `ffmpeg`, `streamlink`, or the built-in Rust engine (`mesio`).
- **Post-processing pipelines** — automatically transcode, segment, or hand off recordings to your own scripts.
- **Web UI + REST API** — manage streamers from the browser; automate with the API (JWT-protected, OpenAPI docs at `/api/docs`).
- **Persistent and reliable** — SQLite-backed state; schema upgrades happen on startup.

## Companion CLI tools

Two standalone tools ship in the same repo:

| Tool | What it's for | Docs |
| --- | --- | --- |
| `strev` | Inspect a live-stream URL and pull out media info from supported platforms. | [`strev-cli/README.md`](./strev-cli/README.md) |
| `mesio` | Download and repair FLV / HLS streams from the command line. | [`mesio-cli/README.md`](./mesio-cli/README.md) |

## Helpful links

- [Configuration reference](https://docs.srec.rs/en/getting-started/configuration)
- [Architecture overview](https://docs.srec.rs/en/concepts/architecture)
- [Engines explained](https://docs.srec.rs/en/concepts/engines)
- [FAQ](https://docs.srec.rs/en/getting-started/faq)

## For developers

This is a Cargo workspace. The main pieces:

- `rust-srec/` — the recorder backend (API, scheduler, pipeline, database)
- `strev-cli/`, `mesio-cli/` — the CLI tools above
- `crates/` — reusable libraries: platform extractors, FLV / HLS / TS parsers, the `mesio` download engine, and stream-repair utilities

Common commands:

```bash
cargo build                       # build everything
cargo test                        # run tests
cargo clippy -- -D warnings       # lint
cargo fmt                         # format
```

Contributions welcome — see [CONTRIBUTING.md](./CONTRIBUTING.md).

## License

Released under the [MIT License](./LICENSE).
