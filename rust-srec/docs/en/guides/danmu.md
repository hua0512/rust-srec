# Record chat and view statistics

## Enable chat recording

1. Check the [platform guide](../platforms/) for danmu support and any login requirements.
2. Turn on **Record Danmu** (`record_danmu`) in global settings, or override it for a platform, template, or streamer.
3. Record a live session and open its session page to inspect chat files and statistics.

Video and chat files are associated by recording segment. A paired-segment workflow waits for both files before processing them. See [workflow triggers](../concepts/pipeline.md#pipeline-triggers) for subtitle conversion and burn-in.

## Watch a recording with its chat

Playing a video from the session page shows its recorded chat over the picture, timed to the video. Chat messages and super chats are shown; gifts are not. The speech-bubble button in the player controls hides or shows the chat; in a narrow player it moves into the settings panel as **Show danmu**. When the screen is full, extra messages are skipped rather than stacked on top of each other.

The sliders button next to it opens **Danmu settings**:

- **Hide**: scrolling, top, bottom, coloured messages, or super chats.
- **Opacity**, **Font size** and **Speed**.
- **Display area**: keep the chat in the top quarter, half or three quarters of the picture, or let it use all of it. A smaller area keeps it off subtitles and thins out a busy chat.
- **Merge repeated comments**: the same message sent several times within 10 seconds appears once, with a count.
- **Pause a comment on hover**: stops a message under the mouse pointer so it can be read, and shows who sent it.
- **Show activity above the progress bar**: a strip showing where the chat is busiest.
- **Blocklist**: hide messages containing a word, or everything from a user name. Words match regardless of case; user names must match exactly.

The settings and blocklist are remembered in this browser and apply to every recording. **Reset** restores the display settings but keeps the blocklist.

The chat only appears while its XML file is kept. A workflow that converts the XML and deletes the original leaves the video without it.

## Configure the summary

Use **Danmu Statistics** to choose ranking lengths, activity intervals, tracking capacity, and extra stop words. These settings follow the same configuration hierarchy as recording settings. [The field reference](../reference/settings.md#danmu-statistics) lists defaults and limits.

Disabling the summary still records chat files, but stops computing and saving the summary containing viewer names. Estimated counts are marked with `≈`. Gifts appear only for platforms that report them.

## Understand live and interrupted sessions

Statistics are saved about once a minute during recording. A restart resumes saved counts; the most recent unsaved interval can be lost. Long sessions reduce timeline resolution to retain the full recording period.

Chat reconnects while video recording continues. A prolonged outage appears in System Health. Messages the platform never delivered cannot be recovered. New XML files filter invalid characters; existing files are not repaired automatically.
