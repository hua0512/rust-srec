# platforms-parser

[![License](https://img.shields.io/crates/l/platforms-parser.svg)](https://github.com/hua0512/rust-srec/blob/main/LICENSE)

This library provides a unified interface to extract streaming media information (like video URLs, titles, and metadata) from various live streaming and video-on-demand platforms.

**Note:** For brevity, the `www.` subdomain is omitted from URLs where it is optional.

## Supported Platforms

| Platform    | Supported URL Type                               | Danmaku |
|-------------|--------------------------------------------------|---------|
| Acfun       | `acfun.cn/live/{room_id}` (also `live.`/`m.` hosts) | |
| Bigo        | `bigo.tv/{room_id}` or `bigo.tv/{lang}/{room_id}` | Yes |
| Bilibili    | `live.bilibili.com/{room_id}`                    | Yes |
| Douyin      | `live.douyin.com/{room_id}`                      | Yes |
| Douyu       | `douyu.com/{room_id}` (numeric)                  | Yes |
| Huya        | `huya.com/{room_id}`                             | Yes |
| PandaTV     | `pandalive.co.kr/play/{user_id}`                 | |
| Picarto     | `picarto.tv/{channel_name}`                      | |
| Redbook     | `xhslink.com/m/{share_id}` or `xhslink.com/o/{share_id}` (profile URLs are rejected) | |
| SOOP        | `play.sooplive.co.kr/{user_id}[/{broadcast_id}]` (also `ch.`/`m.` hosts, `sooplive.com`, `afreecatv.com`) | Yes |
| TikTok      | `tiktok.com/@{username}/live`                    | Yes |
| TwitCasting | `twitcasting.tv/{username}`                      | Yes |
| Twitch      | `twitch.tv/{channel_name}`                       | Yes |
| Weibo       | `weibo.com/u/{user_id}` or `weibo.com/l/wblive/p/show/{live_id}` | |

URLs that match no built-in extractor fall back to a [Streamlink](https://streamlink.github.io/)
extractor when the `streamlink` executable is available. Pass
`ExtractorSelection::Streamlink` to `create_extractor` to force it for any URL.

## Features

* **Unified API**: every platform implements the `PlatformExtractor` trait (`extract`, `get_url`).
* **Asynchronous**: built on `async/await` and `reqwest`.
* **Extensible**: `ExtractorFactory` selects an extractor from a URL regex registry.
* **Cookie management**: each extractor instance keeps its own cookie store.
* **Danmaku**: `danmaku::ProviderRegistry::with_defaults()` provides live chat providers for the platforms marked above.

## Usage

Add `platforms-parser`, `reqwest` and `tokio` to your `Cargo.toml` dependencies.

```rust
use platforms_parser::extractor::factory::{ExtractorFactory, ExtractorSelection};
use reqwest::Client;

#[tokio::main]
async fn main() {
    let url = "https://www.twitch.tv/some_channel"; // Example URL
    let factory = ExtractorFactory::new(Client::new());

    match factory.create_extractor(url, None, None, ExtractorSelection::Auto) {
        Ok(extractor) => match extractor.extract().await {
            Ok(media_info) => {
                println!("Title: {}", media_info.title);
                println!("Artist: {}", media_info.artist);

                if let Some(headers) = &media_info.headers {
                    println!("Headers:");
                    for (key, value) in headers {
                        println!("  {key}: {value}");
                    }
                }

                for (i, mut stream) in media_info.streams.into_iter().enumerate() {
                    // Some platforms resolve the playable URL only in get_url.
                    if let Err(e) = extractor.get_url(&mut stream).await {
                        eprintln!("  Stream {}: {e:?}", i + 1);
                        continue;
                    }
                    println!("  Stream {}:", i + 1);
                    println!("    URL: {}", stream.url);
                    println!("    Quality: {}", stream.quality);
                }
            }
            Err(e) => eprintln!("Extraction failed: {e:?}"),
        },
        Err(e) => eprintln!("Failed to create extractor: {e:?}"),
    }
}
```

`create_extractor(url, cookies, extras, selection)` takes an optional cookie
string and optional platform-specific JSON extras.

> [!NOTE]
> **Additional URL resolution required for some platforms**
>
> Some platforms (e.g., `Huya`, `Douyu`, `Bilibili`) resolve the final, playable stream URL in a second asynchronous call. The `extract` method may return a `StreamInfo` with a temporary or incomplete URL.
>
> Always call `extractor.get_url(&mut stream_info).await` before using the URL. For platforms that do not need this step, the default implementation does nothing.

### Errors

`ExtractorError` includes `Authentication { code }` and
`RateLimited { scope, code, retry_after }`; both `extract` and `get_url` can
return them, so preserve the variants when wrapping errors. Only an explicit
account authentication failure or `ThrottleScope::Account` is evidence that an
account needs recovery. A generic HTTP 401/403, age restriction, or unknown
throttle scope does not establish that an account is invalid. SOOP
login-required and Bilibili login-expired response codes map to typed errors;
ambiguous platform failures remain ordinary errors.

`retry_after` can exceed a caller's default backoff cap and should be honored.
`ExtractorError::check_response` applies the same HTTP classification for
provider adapters; Bilibili `QrLoginError` and `TokenRefreshError` wrap it in a
`Response` variant. QR, token and cookie validation surface HTTP 429 and its
retry delay before decoding the body. `QrGenerateResponse::expires_in` carries
the provider's QR expiry in seconds when the provider sends one.

### Diagnostic privacy

Use `ExtractorError::category()` for logs that must not include upstream bodies
or signed URLs. Extraction diagnostics log response sizes, counts and safe error
categories instead of authentication-bearing bodies, signed URLs, request
builders or cookie values. The `Debug` views of `Extractor` and of QR login
responses redact network and login material.

## License

This project is licensed under either of the following, at your option:

* MIT License
* Apache License, Version 2.0
