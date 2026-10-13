# Release Notes

## `unreleased`

## New features

- **MPEG-DASH recording:** Mesio can now record MPEG-DASH (`.mpd`) streams, both live and finished ones. Video and audio delivered separately are merged into one MP4 file. Choose the video and audio quality, audio language, where a live recording starts, and download limits on the new **DASH Tuning** tab of a Mesio engine, and add `dash` to **Preferred Formats** to prefer DASH when a platform offers it. Encrypted (DRM) streams are not supported, and the web player cannot play DASH streams. See [MPEG-DASH recording](../concepts/engines.md#mpeg-dash).

## Recording and recovery

- Fixed Mesio refusing to record a stream whose address does not show whether it is HLS or FLV. It now uses the format the platform reports.
- Fixed Mesio sometimes leaving segments out of a recording of a finished HLS stream (a replay or VOD). When a segment downloaded slowly or had to be retried while many later ones finished, it was dropped and the recording still ended normally without it. This also applied to high-bitrate streams with large segments.
- Fixed Mesio leaving out repeated segments when recording a finished HLS stream (a replay or VOD). If a playlist listed the same segment more than once, only the last occurrence was recorded.
- When every segment of an HLS stream fails to download, Mesio now reports why, such as the HTTP status or a network error, instead of "stream is empty". When an fMP4 stream's initialization segment cannot be downloaded, Mesio stops downloading the media that depends on it, which could only be thrown away, so such an attempt ends sooner.

## Web interface

- Fixed cards, editors and detail pages overflowing or squeezing their content at laptop and desktop window sizes once the sidebar takes its share. Card lists now fit as many columns as the page has room for, page headers wrap their buttons, and long names, titles, URLs and settings end in "…" with the full text on hover.
- Fixed Mesio engine overrides in templates. Every HLS and FLV setting, and the **Fix FLV Streams**, **Fix HLS Discontinuities** and **Fix DASH Streams** switches, now show the engine setting they inherit and can be set to use that, the default, or a custom value; previously unset switches showed as off, the HLS settings were locked until **Fix HLS Discontinuities** was turned on, and a template containing a reset setting could not be saved. **Disabled** is no longer offered for **Max Stall Duration** and **VOD Segment Timeout** in a template, because a template could never turn them off; set that on the engine instead.
- The **HLS Tuning** tab of a Mesio engine is no longer locked while **Fix HLS Discontinuities** is off, since its download settings apply either way.
- Saving a Mesio engine or template no longer removes HLS settings the form does not show, such as ones set through the API, and a cleared number in a gap strategy, variant or DASH quality choice is now flagged instead of being saved as 0.
- The engines settings page is now a list like proxies, showing each engine's type, installed version and status, with **Configure** beside it.
