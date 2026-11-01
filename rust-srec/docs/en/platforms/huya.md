# Huya

[Huya](https://www.huya.com) (虎牙) is a Chinese game streaming platform.

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
- **Authentication**: Typically **not required**. If specific streams need cookies, add them as an account profile in **Settings** → **Platforms** → **Huya**.
- **Preferred Format**: Huya supports both **FLV** and **HLS**. FLV is generally recommended for recording.
- **Stream Quality**: Turn on **Force Origin Quality** (`force_origin_quality`, off by default) to attempt to get the highest available quality.
- **Extraction API Mode** (`api_mode`): `WEB` (default) reads the room page; `MP` and `WUP` use Huya's app APIs and work only for rooms with numeric IDs. Rooms with non-numeric IDs always use `WEB`.
- **Client Type (ctype)** (`platform`): the client identity sent with stream requests, `huya_pc_exe` by default.
- **CDN Selection**: The extractor automatically attempts to select the best CDN for your location.
:::
