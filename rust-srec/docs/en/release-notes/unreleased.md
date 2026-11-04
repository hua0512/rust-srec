# Release Notes

## `unreleased`

## New features

- **Session page**: recorded chat now plays over the video when you watch a recording, timed to the picture. A **Danmu settings** panel in the player hides message types, sets opacity, font size, speed and how much of the picture the chat covers, merges repeated messages, pauses a message under the mouse, shows where the chat is busiest above the progress bar, and blocks words or users. Settings are remembered, and large chat files load without freezing the page. See [Record chat](../guides/danmu.md#watch-a-recording-with-its-chat).
- **Player page**: live streams now show their chat over the picture as it arrives, with the same show/hide button and **Danmu settings**. Each player in the grid shows its own stream's chat, and a hidden chat is not downloaded. See [Watch a live stream with its chat](../guides/danmu.md#watch-a-live-stream-with-its-chat).
- **Twitch**: new **Allow H.265 (HEVC)** and **Allow AV1** platform settings let you record the higher qualities some channels stream only in those codecs. Both are off by default, so recordings stay H.264 unless you turn them on. See [Twitch](https://docs.srec.rs/en/platforms/twitch).

## Fixes

- **Presets**: opening a saved remux preset or workflow step that encodes with NVIDIA (NVENC) could switch its encoder preset to Medium, and the next save kept that change. The saved preset is now kept.
- **Streamers page**: one streamer whose saved settings could not be read no longer stops the whole streamers list from loading. Those settings show as not configured.
- **Security**: a link crafted by another site could make the stream proxy show a page that acted as the signed-in user. The proxy now only passes on video, audio, images and playlists, and refuses requests from other sites.
- **Security**: a failed sign-in no longer writes the entered password to the server log, and a failed sign-out no longer writes the sign-in token.
- **Sign-in**: a failed sign-in now shows the server's reason instead of an internal server address.
- **FLV recordings**: files recorded with the Mesio engine now list Mesio as their creator instead of an outdated version number, and their real data size instead of 0, so media tools show correct file details.
- **FLV recordings**: with the Mesio engine and **Fix FLV Streams** on, when a stream restarted mid-broadcast, for example after the streamer turned on stream delay, the recording could become one very long file that players showed as 0 seconds long and that was never split by the duration limit. Recordings now keep counting time across such restarts and split as configured.
- **FLV recordings**: browser players can now show the length of a recording and seek in it when the stream sent no file information. This applies to recordings made with the Mesio engine and **Fix FLV Streams** on; FFmpeg and Streamlink recordings are unchanged.
- **FLV recordings**: with the Mesio engine and **Fix FLV Streams** on, recordings of streams that send several conflicting setups before the picture starts now copy and convert correctly. Before, copying or converting such a file could keep the earlier setup instead of the one the stream settled on. FFmpeg and Streamlink recordings are unchanged.
- **Huya**: ordinary chat messages are now recorded without a colour, so they appear white in converted subtitles and burned-in videos instead of blue. Messages Huya colours still keep their colour. Chat files recorded before this update keep the blue.
- **Twitch**: a channel that has just gone live or just ended is now reported as offline until its stream can be played, instead of failing the live check with an error. When Twitch refuses playback, its own error message is shown.
- **Twitch**: a channel that does not exist is now shown as not found, and a suspended channel as banned, instead of failing every live check with a data error.

- **Twitcasting**: the **Stream Password** platform setting now unlocks password-protected broadcasts. Before, only a password added to the streamer URL (`?password=...`) worked; a password in the URL still takes priority.
- **YouTube**: a link to an uploaded or finished video is now reported as offline instead of live, so it is no longer recorded again on every live check. Live broadcasts are unaffected.
