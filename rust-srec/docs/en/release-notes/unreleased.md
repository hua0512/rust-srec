# Release Notes

## `unreleased`

## New features

- **MPEG-DASH recording:** Mesio can now record MPEG-DASH (`.mpd`) streams, both live and finished ones. Video and audio delivered separately are merged into one MP4 file. Choose the video and audio quality, audio language, where a live recording starts, and download limits on the new **DASH Tuning** tab of a Mesio engine, and add `dash` to **Preferred Formats** to prefer DASH when a platform offers it. Encrypted (DRM) streams are not supported, and the web player cannot play DASH streams. See [MPEG-DASH recording](../concepts/engines.md#mpeg-dash).

## Recording and recovery

- Fixed Mesio refusing to record a stream whose address does not show whether it is HLS or FLV. It now uses the format the platform reports.

## Web interface

- Fixed cards, editors and detail pages overflowing or squeezing their content at laptop and desktop window sizes once the sidebar takes its share. Card lists now fit as many columns as the page has room for, page headers wrap their buttons, and long names, titles, URLs and settings end in "…" with the full text on hover.
- The engines settings page is now a list like proxies, showing each engine's type, installed version and status, with **Configure** beside it.
