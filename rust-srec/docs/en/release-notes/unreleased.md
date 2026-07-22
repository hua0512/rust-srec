# Release Notes

## `unreleased`

### Fixes

- **Pipeline execution details display correctly on mobile**

  On narrow screens, the summary cards on a pipeline execution's detail page could overlap their icons and cut off long values like the progress percentage. The cards now adapt to smaller screens so progress, step counts, and start time stay readable.

- **Theme changes apply immediately again**

  Picking a new theme on the Themes page took effect only after refreshing the page if you had already customized the theme before. Theme presets, colors, and radius changes now apply instantly, as they should.

- **List filters and search stay put when you navigate back**

  On pages like Streamers, Sessions, Pipeline Jobs, Presets, Workflows, and Media Outputs, your search text, filters, sort order, and page position were reset whenever you opened an item and came back, or refreshed the page. They are now kept in the page address, so going back or reloading keeps your place — and you can bookmark or share a filtered view.

- **Pagination buttons now follow your language**

  On paginated lists, the "Previous" and "Next" buttons stayed in English even when the interface was set to another language. They now appear in your selected language.

- **Streams that need a login play reliably in the built-in player**

  Live streams whose playlists require cookies or custom headers could stop working after the first request, so playback stalled or failed in the web and desktop player. The player now carries those headers through the whole playlist — quality variants, segments, encryption keys, and low-latency parts — so these streams play consistently.

- **New setting for stream sources on your own network**

  To keep the built-in player's proxy from being pointed at private addresses, it now only reaches public stream sources by default. If you watch or record from a source on your own network — a LAN restreamer, a camera, or a device on your tailnet — turn on **Allow private stream proxy targets** under Network & System settings to permit it.

- **Interrupted rclone move uploads now finish on retry**

  When a move upload sent some files and then failed partway — after a network hiccup, for example — retries kept failing because the files that were already uploaded no longer existed locally, and retrying the job by hand hit the same error. Retries now pick up where the upload left off and only send the remaining files, and a retried job whose files were all uploaded earlier completes successfully.

- **Interrupted local file moves also recover on retry**

  The same applied to the copy/move pipeline step: if a move was interrupted after some files had already reached the destination folder, retrying the job reported those files as failed — or failed the whole job when every file had already been moved. Retries now recognize files that already arrived at the destination and complete normally. Moves across drives also copy to a temporary name first, so an interrupted move can no longer leave a half-written file under the final name.

- **Cancelled or timed-out jobs no longer leave tools running in the background**

  When a pipeline job was cancelled or ran past the job timeout, the external tool it had launched — an rclone transfer, ffmpeg processing such as remuxing, transcoding, subtitle burn-in or thumbnails, danmaku conversion, or a Telegram download — could keep running in the background even though the job was already marked failed. rclone's temporary file lists could also pile up in the recording folder. Stopping the job now also stops the tool and removes those temporary files.
