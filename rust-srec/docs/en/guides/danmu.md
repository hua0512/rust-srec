# Record chat and view statistics

## Enable chat recording

1. Check the [platform guide](../platforms/) for danmu support and any login requirements.
2. Enable `record_danmu` in global settings, or override it for a platform, template, or streamer.
3. Record a live session and open its session page to inspect chat files and statistics.

Video and chat files are associated by recording segment. A paired-segment workflow waits for both files before processing them. See [workflow triggers](../concepts/pipeline.md#pipeline-triggers) for subtitle conversion and burn-in.

## Configure the summary

Use **Danmu Statistics** to choose ranking lengths, activity intervals, tracking capacity, and extra stop words. These settings follow the same configuration hierarchy as recording settings. [The field reference](../reference/settings.md#danmu-statistics) lists defaults and limits.

Disabling the summary still records chat files, but stops computing and saving the summary containing viewer names. Estimated counts are marked with `≈`. Gifts appear only for platforms that report them.

## Understand live and interrupted sessions

Statistics are saved about once a minute during recording. A restart resumes saved counts; the most recent unsaved interval can be lost. Long sessions reduce timeline resolution to retain the full recording period.

Chat reconnects while video recording continues. A prolonged outage appears in System Health. Messages the platform never delivered cannot be recovered. New XML files filter invalid characters; existing files are not repaired automatically.
