# Douyu

[Douyu](https://www.douyu.com) (斗鱼) is a major Chinese game streaming platform.

## URL Format

```
https://www.douyu.com/{room_id}
```

## Features

- ✅ FLV streams
- ✅ Danmaku collection
- ✅ Multiple quality options (via `rate` selection)
- ✅ CDN selection support
- ✅ Interactive game stream detection

::: info
- **Authentication**: Cookies are generally **not required** for most streams. If you encounter "Login required" or need to record VIP-only streams, set cookies in **Settings** → **Platform** → **Douyu**.
- **Preferred Format**: Douyu primarily uses **FLV** for live streams.
- **Quality Control**: Use the `rate` setting to choose quality (0 for source/original).
- **CDN Switching**: You can specify a preferred CDN in the configuration if you face buffering issues.
- **Interactive Games**: You can choose to automatically skip "Interactive Games" recordings using `disable_interactive_game`.
:::
