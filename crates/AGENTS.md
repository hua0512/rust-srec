# Library crates AGENTS.md

These notes apply to the library crates under `crates/`. Repository-wide rules still apply. Package names differ from directory names in three places: `crates/mesio` is `mesio-engine`, `crates/platforms` is `platforms-parser`, and the `mesio` package is the CLI in `mesio-cli/`.

## Dependency layers

Lower layers never depend on higher ones. Changing a lower crate affects every consumer above it, so widen validation accordingly.

| Layer | Crates | Purpose |
| --- | --- | --- |
| Primitives | `bytes-util`, `expgolomb`, `media-types`, `amf0`, `tars-codec`, `process-utils` | Bit/byte readers, Exp-Golomb, shared `Resolution`/`SplitReason`/codec info, AMF0, Tencent TARS codec (Huya), child-process helpers |
| Codecs | `aac`, `h264`, `h265`, `av1` | Bitstream headers and parameter sets |
| Containers | `flv`, `ts`, `mp4`, `hls` | FLV tag/header parsing and writing (incl. Enhanced RTMP, see `flv/enhanced-rtmp-v2.md`), MPEG-TS packets/PSI/PES/SCTE-35, ISOBMFF/fMP4 boxes and track merging, HLS segment data (`HlsData`, TS/fMP4 segments, stream profiles) |
| Processing | `pipeline-common`, `flv-fix`, `hls-fix` | Generic processor pipeline and writer task; FLV and HLS repair/split operators |
| Engine | `mesio` (`mesio-engine`) | HLS, MPEG-DASH and FLV downloader |
| Extraction | `platforms` (`platforms-parser`) | Platform stream URL extraction and danmaku (live chat) |

Consumers: `rust-srec` uses every processing/engine/extraction crate; `mesio-cli` uses the engine and processing crates; `strev-cli` uses only `platforms-parser`.

## mesio-engine (`crates/mesio`)

Design doc: `docs/HLS_ENGINE_ARCHITECTURE.md` (segment engine; read before changing scheduling, store or assembly). `README.md` shows the public API and the session contract (guaranteed `items` vs. best-effort `events`).

- `session.rs`: `MesioDownloader` and per-download `DownloadSession<T>`: guaranteed `items` stream, best-effort `events` stream, cancellation `handle`.
- `protocol_builder.rs`, `builder.rs`, `config.rs`, `source.rs`: protocol builders, shared config, source selection and fallback.
- `segment_engine/` (crate-private): protocol-neutral core shared by HLS and DASH. `identity.rs` (segment identity), `store.rs` (authoritative lifecycle state), `reactor.rs` (scheduling), `budget.rs`/`throttle.rs` (byte budgets, backpressure), `fetch.rs` (download + decrypt), `assembler.rs` (ordered output).
- `hls/`: playlist watcher and planner in `engine/`, AES decryption, platform quirks (`twitch_processor.rs`, `soop_processor.rs`).
- `dash/`: `manifest/` is pure MPD parsing/resolution with no I/O (`parse.rs`, `resolve.rs`, `timeline.rs`, `template.rs`, `clock.rs`); `watcher.rs` refreshes MPDs; `planner.rs` picks segments per track; `mux.rs` muxes video and audio into one fMP4 stream emitted as `hls::HlsData`.
- `flv/`: FLV HTTP downloader.
- `cache/`, `rate_limit.rs`, `proxy.rs`, `redact.rs`: caching, rate limiting, proxy config, URL/secret redaction for logs.
- Tests: `tests/hls_engine.rs`, `tests/dash_engine.rs`, `tests/dash_manifest.rs` with local fixtures.

Design decisions (ask before reversing):
- No ffmpeg dependency for muxing and no third-party MP4/MPD crate: extend `crates/mp4`, citing the spec clause, fuzzed, never panicking.
- DRM (CENC) is out of scope; protected content fails with a typed error.
- Known limits kept on purpose: HLS progress counts bytes from failed attempts (real network traffic); `DownloadError::StreamNetwork` keeps a string reason because rust-srec constructs it; the opt-in `vod_segment_timeout` only arms once a later segment has arrived; one slow VOD segment can hold the 80% byte window (skipping instead used to drop segments still in flight); a DASH encoder restart that reuses segment numbers ends through the 60 s stall watchdog; a video-only DASH Period waits up to `max_audio_lag` and reports `TrackStalled` (see `dash/mux.rs`).

## platforms-parser (`crates/platforms`)

- `extractor/factory.rs`: the `platform_registry!` table mapping URL regexes to extractors, and `ExtractorSelection` (built-in vs. streamlink fallback in `streamlink_extractor.rs`).
- `extractor/platform_extractor.rs`: the `PlatformExtractor` trait every platform implements.
- `extractor/batch_status.rs`: the optional `BatchStatusProbe` trait (live/offline for many channels in one anonymous request), found with `factory::batch_probe_for_url`; implementations in `platforms/{bilibili,twitch}/batch_status.rs`, tested against `serve_once` in `extractor/test_support.rs`.
- `extractor/platforms/<name>/`: one directory per platform. `builder.rs` is the extractor, `models.rs` holds API response types, `danmu.rs` is the chat provider; signing/auth helpers live beside them (`douyin/abogus.rs`, `bilibili/wbi.rs`, `douyu/app_sign/`, `huya/tars/`).
- `danmaku/`: provider trait, `registry.rs` (`ProviderRegistry::with_defaults` lists chat providers), shared WebSocket provider, XML writer, statistics.
- `media/`: `MediaInfo` and `StreamInfo` returned by extractors.
- Tests that hit live platforms are `#[ignore]`d; keep new ones that way.
- Platform notes:
  - TikTok's `cmaf` stream is a live DASH MPD and is opt-in (`format_opt_in`); FLV stays the default.
  - Douyu: the App endpoint gets origin quality only with the `token` built from the account's `acf_*` cookies (`app_token()` in `douyu/app.rs`). Signed-in Web needs the session cookies on `getEncryption` too, or signing returns 403. Signed-out Web URLs carry a signed `expire=300`, so don't rewrite URL parameters. `live_signed_in_playback` (ignored; set `DOUYU_COOKIES` and `DOUYU_LIVE_URL`) checks both paths against a real account.

Adding a platform touches, besides its directory: `extractor/platforms.rs`, the registry in `extractor/factory.rs` (a batch status probe, if any, attaches to the platform's entry with `; batch: PROBE`), `extractor/platform_configs.rs` if it has settings, `danmaku/registry.rs` if it has chat, `crates/platforms/README.md`, `strev-cli/src/commands.rs` (`list_platforms`), a backend migration seeding its `platform_config` row (e.g. `rust-srec/migrations/20260715000000_add_soop_platform.sql`) and `rust-srec/src/proxies/probe.rs`, frontend `src/lib/utils.ts`, `src/components/pipeline/constants.tsx` and `src/api/schemas/platform-configs.ts`, the docs pages under `rust-srec/docs/{en,zh}/platforms/`, and the platform lists in the root `README.md`/`README.zh-CN.md`.

## Processing crates

- `pipeline-common`: `Processor<T>` and `Pipeline<T>` (`processor.rs`, `pipeline.rs`), threaded pipelines (`channel_pipeline.rs`), generic writer task with file rotation (`writer_task.rs`), manual split control, progress reporting, `StreamerContext` (cancellation and shared state).
- `flv-fix`: `FlvPipeline::build_pipeline` in `pipeline.rs` fixes the operator order, which is significant: defragment → header check → split → GOP sort → duplicate filter (optional) → time consistency → timing repair → limit → time consistency → keyframe filler (optional) → script filter (the last two are skipped in pipe mode). Operators are in `operators/`, one per file. `writer_task.rs` owns FLV output files and patches `onMetaData` (duration, keyframe index) through `amf/` when a file closes. `script_modifier.rs` is a public post-hoc file patcher with no in-repo callers.
- `hls-fix`: operators in `operators/` (defragment, segment split, segment limiter), `pipeline.rs`, and `writer_task.rs` for TS/fMP4 output.

## Backend touchpoints

Backend consumers live in `rust-srec/src/downloader/engine/` (per-engine adapters, `mesio/` for this engine), `rust-srec/src/danmu/` (danmaku service), `rust-srec/src/credentials/platforms/` (platform login/cookie refresh), `rust-srec/src/monitor/` (live checks via extractors) and `rust-srec/src/proxies/`.
