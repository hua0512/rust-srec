# platforms-parser

[![License](https://img.shields.io/crates/l/platforms-parser.svg)](https://github.com/hua0512/rust-srec/blob/main/LICENSE)

This library provides a unified interface to extract streaming media information (like video URLs, titles, and metadata) from various live streaming and video-on-demand platforms.

**Note:** For brevity, the `www.` subdomain is omitted from URLs where it is optional.

## Supported Platforms

| Platform    | Supported URL Type                               |
|-------------|--------------------------------------------------|
| Bilibili    | `live.bilibili.com/{room_id}`                    |
| Douyin      | `live.douyin.com/{room_id}`                      |
| Douyu       | `douyu.com/{room_id}`                            |
| Huya        | `huya.com/{room_id}`                             |
| PandaTV     | `pandalive.co.kr/play/{user_id}` (Defunct)       |
| Picarto     | `picarto.tv/{channel_name}`                      |
| Redbook     | `xiaohongshu.com/user/profile/{user_id}` or `xhslink.com/{share_id}` |
| TikTok      | `tiktok.com/@{username}/live`                   |
| TwitCasting | `twitcasting.tv/{username}`                      |
| Twitch      | `twitch.tv/{channel_name}`                       |
| Weibo       | `weibo.com/u/{user_id}` or `weibo.com/l/wblive/p/show/{live_id}` |

## Features

* **Broad Platform Support**: Extract streaming data from a wide range of popular live streaming and video platforms.
* **Unified API**: A single, easy-to-use interface (`extract`, `get_url`) for all supported platforms, simplifying your code.
* **Asynchronous by Design**: Built with `async/await` for non-blocking, high-performance network operations suitable for concurrent applications.
* **Extensible Architecture**: The underlying factory pattern and `PlatformExtractor` trait make it straightforward to add support for new platforms.
* **Built-in Cookie Management**: Each extractor instance maintains its own cookie store, providing robust support for platforms that require authentication or session management.

## Usage

Add `platforms-parser` and `reqwest` to your `Cargo.toml` dependencies.

```rust
use platforms_parser::extractor::factory::ExtractorFactory;
use reqwest::Client;

#[tokio::main]
async fn main() {
    let url = "https://www.twitch.tv/some_channel"; // Example URL
    let client = Client::new();
    let factory = ExtractorFactory::new(client);

    match factory.create_extractor(url, None, None) {
        Ok(extractor) => {
            match extractor.extract().await {
                Ok(media_info) => {
                    println!("Title: {}", media_info.title);
                    println!("Uploader: {}", media_info.uploader);
                    for (i, mut stream) in media_info.streams.into_iter().enumerate() {
                        // For some platforms, you need to call get_url to get the real stream url
                        extractor.get_url(&mut stream).await.unwrap();
                        println!("  Stream {}:", i + 1);
                        println!("    URL: {}", stream.url);
                        println!("    Quality: {}", stream.quality);
                    }
                }
                Err(e) => {
                    eprintln!("Extraction failed: {:?}", e);
                }
            }
        }
        Err(e) => {
            eprintln!("Failed to create extractor: {:?}", e);
        }
    }
}
```

> [!NOTE]
> **Additional URL resolution required for some platforms**
>
> Some platforms (e.g., `Huya`, `Douyu`, `Bilibili`) require an additional asynchronous call to resolve the final, playable stream URL. The `extract` method may return a `StreamInfo` object with a temporary or incomplete URL.
>
> Always call `extractor.get_url(&mut stream_info).await` to ensure you have the correct, final URL before attempting to use it. For platforms that do not require this step, the default implementation will do nothing.

## License

This project is licensed under either of the following, at your option:

* MIT License
* Apache License, Version 2.0
