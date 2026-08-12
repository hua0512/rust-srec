# Release Notes

## `unreleased`

## Pipeline and uploads

- **Upload recordings to Baidu Netdisk**

  A new `baidupcs` pipeline processor uploads recordings to Baidu Netdisk through the BaiduPCS-Go command-line tool, which is now bundled in the Docker image. Add it to a pipeline like any other upload step: the destination folder supports the usual streamer/title/date placeholders, same-name files can be skipped or overwritten, and uploads appear in the same live progress, per-file records and streamer-card indicators as rclone transfers. Log in from the preset editor — paste your netdisk cookies (or BDUSS and STOKEN) once and the account card shows who is signed in and how much space is left. Tick **Remember for automatic re-login** and upload jobs log in again by themselves when the session expires, so a recording made at night still lands in the netdisk without anyone clicking Login; leave it unticked and the credentials are handed to BaiduPCS-Go without the app keeping them. If the remembered credentials themselves stop working, a notification tells you to log in again and further attempts pause for an hour instead of hammering Baidu. Logging out forgets the remembered credentials. Because BaiduPCS-Go's exit code does not reflect upload results, rust-srec reads the tool's per-file output instead, and a retried job re-sends only the files that did not make it. See [DAG Pipeline](../concepts/pipeline.md#baidu-netdisk-baidupcs).

## Danmu

- **Live danmu statistics while recording**

  Danmu statistics no longer wait for the stream to end: while a recording is running, a snapshot is saved about once a minute, so the session page's danmu panel (totals, activity timeline, top talkers, frequent words) fills in while the stream is still live. If the app crashes or the host reboots mid-recording, at most the last minute of statistics is lost instead of the whole session's.

- **Activity timeline covers the whole stream on long sessions**

  The danmu activity chart used to keep only the most recent six hours at full detail and silently dropped the oldest points on longer sessions. Once the limit is reached it now halves the chart's resolution instead, so a 12-hour recording still charts from the first minute to the last — just at a coarser granularity.

- **Expandable Top Talkers**

  The Top Talkers card on the session page shows the six most active chatters by default and can now be expanded to the full ranking (up to 32 users are tracked per session).

- **Better word splitting for Chinese chat**

  The frequent-words statistic now treats full-width punctuation (`,` `。` `!` `?` ...), symbols and emoji as word separators. Previously only spaces and ASCII punctuation split words, so a Chinese message without spaces was counted as one giant "word".

- **Unique chatters metric**

  The session danmu panel now shows how many distinct users chatted during the stream (a memory-bounded estimate, accurate to about 1–2%), alongside the total message count. Sessions recorded before this release show a dash.

- **Gift rankings**

  For platforms that report gifts in chat (Bilibili, Douyu, Bigo, SOOP, ...), the session page now shows two extra charts: the top gift senders and the most-sent gifts, both weighted by the number of gift items rather than messages. The charts only appear when the stream actually received gifts.

- **Removed the `danmu_sampling_config` setting**

  This template/streamer setting never had any effect — statistics have always counted every message. The field has been removed from the REST API (`/api/templates`) and the database; existing configurations are cleaned up automatically, and older exports that still contain the field import fine.
