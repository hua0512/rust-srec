# rust-srec

<p align="center">
  <img src="rust-srec/docs/public/stream-rec.svg" width="88" alt="rust-srec logo" />
</p>

A production-ready, automated stream recording solution built with Rust.

- Documentation: https://hua0512.github.io/rust-srec/
- GitHub: https://github.com/hua0512/rust-srec

## Quick links

- Get started: https://hua0512.github.io/rust-srec/en/getting-started/
- Docker deployment: https://hua0512.github.io/rust-srec/en/getting-started/docker
- Installation (binaries / from source): https://hua0512.github.io/rust-srec/en/getting-started/installation
- Configuration: https://hua0512.github.io/rust-srec/en/getting-started/configuration
- Architecture overview: https://hua0512.github.io/rust-srec/en/concepts/architecture

## What’s in this repo

This is a Rust workspace containing:

- `rust-srec/` (package: `rust-srec`): the recorder backend (REST API + scheduler + pipeline + DB)
- `strev-cli/` (package: `strev`): a CLI for extracting/inspecting stream media info from platforms
- `mesio-cli/` (package: `mesio`): a CLI for downloading/fixing FLV/HLS streams and files
- `crates/`: reusable protocol/container crates (FLV/HLS/TS) and platform extractors

## Highlights

- Multi-platform support via `platforms-parser` (12 platforms): acfun, bilibili, douyin, douyu, huya,
  pandatv (legacy), picarto, redbook, tiktok, twitcasting, twitch, weibo
- Automatic recording when streamers go live, with persistent state in SQLite (migrations run at startup)
- Post-processing DAG pipelines (segment, paired-segment, session-complete) with built-in processors
- Danmaku/chat capture alongside video (where supported by the platform)
- Multiple download engines: `ffmpeg`, `streamlink`, and the built-in Rust engine `mesio`
- REST API with OpenAPI + Swagger UI (`/api/docs`) and JWT authentication
- Docker-first deployment (official images in `rust-srec/docker-compose.yml`)

Note: Docker images are tagged as `vX.Y.Z` (leading `v`). If you set `VERSION` in `rust-srec/docker-compose.yml`, use `v0.1.0` (not `0.1.0`).

## CLI tools

- `strev` (source in `strev-cli/`): platform extraction / stream inspection
  - Docs: `strev-cli/README.md`
  - Build: `cargo build --release -p strev`
- `mesio` (source in `mesio-cli/`): download + repair FLV/HLS (and pipe output to other tools)
  - Docs: `mesio-cli/README.md`
  - Build: `cargo build --release -p mesio`

## Workspace crates (selected)

- `crates/platforms` (crate: `platforms-parser`): platform URL parsing/extraction + danmaku providers
- `crates/mesio` (crate: `mesio-engine`): Rust download engine for FLV/HLS with retries, caching, proxy
- `crates/flv`, `crates/hls`, `crates/ts`: container/protocol parsing and helpers
- `crates/flv-fix`, `crates/hls-fix`: stream repair and post-processing utilities

## Development

Common commands:

- Build: `cargo build`
- Test: `cargo test`
- Lint: `cargo clippy -- -D warnings`
- Format: `cargo fmt`

## License

Licensed under either of:

- Apache License, Version 2.0
- MIT license
