# platforms-parser

[![Crates.io](https://img.shields.io/crates/v/platforms-parser.svg)](https://crates.io/crates/platforms-parser)
[![License](https://img.shields.io/crates/l/platforms-parser.svg)](https://github.com/hua0512/rust-srec/blob/main/LICENSE)

This library provides a unified interface to extract streaming media information (like video URLs, titles, and metadata) from various live streaming and video-on-demand platforms.

## Supported Platforms

| Platform    | Supported URL Type                               |
|-------------|--------------------------------------------------|
| Bilibili    | `live.bilibili.com/{room_id}`                    |
| Douyin      | `live.douyin.com/{room_id}`                      |
| Douyu       | `www.douyu.com/{room_id}`                        |
| Huya        | `www.huya.com/{room_id}`                         |
| PandaTV     | `www.pandalive.co.kr/play/{user_id}` (Defunct)   |
| Picarto     | `picarto.tv/{channel_name}`                      |
| Redbook     | `www.xiaohongshu.com/user/profile/{user_id}` or `xhslink.com/{share_id}` |
| TikTok      | `www.tiktok.com/@{username}/live`               |
| TwitCasting | `twitcasting.tv/{username}`                      |
| Twitch      | `twitch.tv/{channel_name}`                       |
| Weibo       | `weibo.com/u/{user_id}` or `weibo.com/l/wblive/p/show/{live_id}` |

## Usage

TODO: Add usage examples.

## Features

The `douyu` feature is enabled by default.

## License

This project is licensed under either of the following, at your option:

*   MIT License
*   Apache License, Version 2.0