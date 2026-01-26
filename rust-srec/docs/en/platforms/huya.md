# Huya

[Huya](https://www.huya.com) (虎牙) is a leading Chinese game streaming platform.

## URL Format

```
https://www.huya.com/{room_id}
```

## Features

- ✅ FLV and HLS streams
- ✅ Danmaku collection
- ✅ Multiple quality options
- ✅ Automatic CDN selection

::: info
- **Authentication**: Typically **not required**. Set cookies in **Settings** → **Platform** → **Huya** if necessary for specific streams.
- **Preferred Format**: Huya supports both **FLV** and **HLS**. FLV is generally recommended for recording.
- **Stream Quality**: `force_origin_quality` is enabled by default to attempt to get the highest available quality.
- **Protocols**: Supports `use_wup` and `use_wup_v2` (default) for stream extraction. Note that **WUP only supports rooms with numeric IDs**.
- **CDN Selection**: The extractor automatically attempts to select the best CDN for your location.
:::
