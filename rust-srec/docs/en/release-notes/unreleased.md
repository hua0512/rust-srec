# Release Notes

## `unreleased`

## New features

- **Twitch**: new **Allow H.265 (HEVC)** and **Allow AV1** platform settings let you record the higher qualities some channels stream only in those codecs. Both are off by default, so recordings stay H.264 unless you turn them on. See [Twitch](https://docs.srec.rs/en/platforms/twitch).

## Fixes

- **Twitch**: a channel that has just gone live or just ended is now reported as offline until its stream can be played, instead of failing the live check with an error. When Twitch refuses playback, its own error message is shown.
- **Twitch**: a channel that does not exist is now shown as not found, and a suspended channel as banned, instead of failing every live check with a data error.

- **Twitcasting**: the **Stream Password** platform setting now unlocks password-protected broadcasts. Before, only a password added to the streamer URL (`?password=...`) worked; a password in the URL still takes priority.
