# Release Notes

## `unreleased`

### Features

- **Choose how a stream's address is looked up**

  Each site normally has its own built-in support for finding the live stream. When a site changes and that support stops working, you can now switch to Streamlink instead, under **Stream lookup**. Set it for one streamer, for a whole site, in a template, or as the default for everything, and the most specific setting wins. It needs the Streamlink tool installed. This is separate from the download engine, which only decides how the stream is saved once its address is known.

- **Per-streamer platform options**

  A new **Platform options** tab on a streamer lets you set that site's own options — quality, login details, Streamlink arguments and so on — for just that streamer, instead of only site-wide. Anything you leave alone keeps coming from the site's settings.

- **Rebuilt the add and edit streamer pages**

  Both pages have been redesigned. Adding a streamer now asks for the link first, confirms the site is recognized, and only then asks for settings — and it no longer asks for the link and name a second time. The edit page keeps everything in one place with the recording history and recent sessions alongside. Leaving either page with unsaved changes now asks first.

- **Clearer settings pages**

  The global settings, notification, recording engine and recording filter pages have been reworked. Related settings sit under headings that say what the group is for, more fields explain themselves when you hover the question mark beside them, and boxes and dropdowns line up with each other. A few settings that were only described in a tooltip now say what they do on the page. The save button no longer sits there greyed out: it appears once you actually change something. None of the settings themselves have changed.

### Fixes

- **Text that stayed in English when using another language**

  In the recording filter editor, the day-of-week tooltips and the day and time preset menus were always shown in English. The priority labels on notification events and channels were too. They now follow the language you picked.

  Notification events also went by their internal names, so the events list, the type filter and the per-channel event pickers showed entries like "STREAM ONLINE". They now read as ordinary text in your language, and searching the event list still matches either wording.

- **Dropdowns are the height they were meant to be**

  Dropdowns across the pipeline step, engine and preset forms were rendering shorter than the boxes next to them, so rows of settings did not line up. They are now the intended height everywhere.

- **Dropdowns show the saved value when a page opens**

  On several pipeline step, engine and job forms, a dropdown could come up empty or on the wrong entry until you touched it, even though a value was saved. They now show what is stored as soon as the page loads.

- **Changing a streamer's link now switches it to the right site**

  If you edited a streamer and replaced its link with one from a different site, the streamer kept using the old site's settings — its cookies, proxy, output folder and danmaku options — and recordings were still filed under the old site's name. The site is now worked out from the link itself, so changing the link moves the streamer across properly. It is no longer something you pick by hand: when adding or editing a streamer, the detected site is simply shown next to the link, and links that no site recognizes are flagged before you save. Streamers that already drifted onto the wrong site are corrected the next time you save them.

- **Deleting a pipeline now updates the job statistics**

  Deleting a pipeline from the Pipeline Jobs page removed the pipeline itself but quietly left its jobs behind, so the Pending, Completed, and Failed counters at the top of the page never went down — deleted failed pipelines kept counting toward the Failed number forever. Deleting a pipeline now also removes its jobs, and the counters reflect it immediately. This also applies to pipelines created before the upgrade, and job records that earlier deletions had already left behind are removed once when you upgrade.

- **Pipeline execution details display correctly on mobile**

  On narrow screens, the summary cards on a pipeline execution's detail page could overlap their icons and cut off long values like the progress percentage, and the card icons sat at uneven heights. The cards now adapt to smaller screens and their icons line up consistently, so progress, step counts, and start time stay readable.

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
