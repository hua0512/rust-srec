# Release Notes

## `unreleased`

This update rebuilds how you add and edit a streamer, and makes what the app is doing far more visible while it does it. A streamer's site is now worked out from its link instead of being picked by hand, each streamer can override its site's own options, and when a site's built-in support for finding the live stream stops working you can fall back to Streamlink without changing anything else. Uploads are no longer a black box: you can watch them run from the streamer's card and see afterwards exactly which files went where. Email notifications work for the first time, at both ends: the SMTP delivery is implemented, and the channel is no longer locked behind a "Coming soon" label.

Behind that, a review went through recording, monitoring, pipelines, the database and the web interface, and most of what came out of it is things you should simply never run into again: streamers that quietly stopped being monitored after a restart, recording size limits that some streams ignored, parts of an HLS recording that were dropped at the end, pipeline steps that left a half-written file at the destination, and a cancelled step that left the rest of its run stuck forever.

The web interface loads faster and holds on to more of what you were doing — your filters, your place in a list, and your session through a network hiccup. The settings, notification, recording engine and recording filter pages have been reorganized so related options sit together, and a few settings that were only explained in a tooltip now say what they do on the page.

Sign-in is stricter as well. The server refuses to start without its signing secret instead of running with an empty one, disabled accounts and forced password changes are enforced on every request rather than only at login, and credentials no longer reach the log files.

## Before you upgrade

- **The server no longer starts with an empty signing secret**

  `JWT_SECRET` has always been documented as required, but an empty value was accepted and used as an empty signing key. The server now refuses to start without it. For local use only, you can set `AUTH_DISABLED=true` together with `API_BIND_ADDRESS=127.0.0.1` (or `::1`); any other bind address rejects that opt-out. The web interface likewise requires `SESSION_SECRET` in production instead of falling back to a built-in value. Setups created from the provided Docker Compose file already have both.

- **Importing a backup is all-or-nothing**

  An import that failed partway used to leave behind whatever it had already written. It now runs as a single transaction and rolls back completely if anything is rejected. Validation is stricter, so a bundle exported by an older version that relied on missing fields being filled in for it may be refused — re-export it from this version. Replacing an existing setup also handles streamer links that differ only in capitalization.

## Streamers and sites

- **Choose how a stream's address is looked up**

  Each site normally has its own built-in support for finding the live stream. When a site changes and that support stops working, you can now switch to Streamlink instead, under **Stream lookup**. Set it for one streamer, for a whole site, in a template, or as the default for everything, and the most specific setting wins. It needs the Streamlink tool installed. This is separate from the download engine, which only decides how the stream is saved once its address is known.

- **Per-streamer platform options**

  A new **Platform options** tab on a streamer lets you set that site's own options — quality, login details, Streamlink arguments and so on — for just that streamer, instead of only site-wide. Anything you leave alone keeps coming from the site's settings.

- **Rebuilt the add and edit streamer pages**

  Both pages have been redesigned. Adding a streamer now asks for the link first, confirms the site is recognized, and only then asks for settings — and it no longer asks for the link and name a second time. The edit page keeps everything in one place with the recording history and recent sessions alongside. Leaving either page with unsaved changes now asks first.

- **Changing a streamer's link now switches it to the right site**

  If you edited a streamer and replaced its link with one from a different site, the streamer kept using the old site's settings — its cookies, proxy, output folder and danmaku options — and recordings were still filed under the old site's name. The site is now worked out from the link itself, so changing the link moves the streamer across properly. It is no longer something you pick by hand: when adding or editing a streamer, the detected site is simply shown next to the link, and links that no site recognizes are flagged before you save. Streamers that already drifted onto the wrong site are corrected the next time you save them.

- **A closed room is reported as offline, not as an error**

  When a room was closed or a broadcast had just ended, several sites replied in a shape the app didn't expect, so the check failed outright — and a streamer whose checks keep failing gets backed off. AcFun, Bilibili, Douyu, PandaTV and Twitcasting now read those replies for what they are and report the streamer as simply offline, or show the site's own message. Alongside that, RedBook titles and nicknames containing the word "undefined" no longer break the check, Picarto reads the channel name from the link even when it carries a query string, and a Twitcasting stream whose first quality fails no longer discards the other qualities.

- **Fewer sign-in requests to the sites you record**

  Bigo and Douyin credentials are now shared across checks with a proper expiry, instead of being requested again on every polling cycle or kept forever once obtained. Several checks starting at once share a single refresh rather than each asking the site for its own, and a credential that failed to refresh is retried next time instead of a stale placeholder being kept in its place. Weibo's fallback-cookie warning is also logged once per run rather than on every check.

## Notifications

- **Email notifications work end to end**

  The email channel was missing a half at each end. The server built its message and then wrote a line to the log saying it would have sent it, and in the interface **Email** was listed as *Coming soon* and could not be picked — even though the form behind that label was complete. Both are fixed. Email now delivers over SMTP, with implicit TLS on port 465 and STARTTLS otherwise, a plain-text and an HTML version of each message, and the same retry, dead-letter and circuit-breaker handling the other channels already had. You can create and edit an email channel from the notification settings like any other type. Relays that need no credentials are supported — leave the username and password empty; filling in only one of the two is rejected in the form rather than failing later at send time.

  Discord is still marked *Coming soon* and remains unavailable.

- **Set the language for each notification channel**

  Notification channels have a new **Notification language** setting, so the alerts sent to a Telegram chat, a webhook or a shared mailbox can each be written in their own language. Channels left on **Same as server** keep using the language the server runs in, so nothing changes unless you pick one. The setting is about the people reading the messages, not about the language you use in this interface.

  Browser and desktop notifications are not covered by this and still follow the server's language.

## Uploads

- **See your uploads happen — and where the files went**

  Uploads used to be a black box: once an upload step finished, nothing recorded where the files had gone, and while one was running there was no sign of it outside the job page. Now, while files are uploading, the streamer's card shows a small cloud badge with live progress — hover it for per-upload speed and size. When the upload finishes, the job's page lists every file with its destination, size and result, including which files failed and why, and the Media Outputs page marks uploaded files with a cloud badge that shows the remote destination on hover. These records are kept after the upload completes, survive restarts, and a retried upload updates them in place.

- **Rclone upload progress now actually updates**

  While an rclone upload ran, the progress area on the job's page never showed anything — no percentage, speed, or time remaining — even though the transfer itself was fine. The statistics rclone was asked to report never actually arrived. Progress now updates once a second with the percentage, transferred size, speed, and time remaining, and the same live numbers drive the upload badge on the streamer's card. If you had added `--progress` (or `-P`) to an rclone step's extra arguments, it is now removed automatically — it would break this reporting — with a note in the job's log.

- **Interrupted rclone move uploads now finish on retry**

  When a move upload sent some files and then failed partway — after a network hiccup, for example — retries kept failing because the files that were already uploaded no longer existed locally, and retrying the job by hand hit the same error. Retries now pick up where the upload left off and only send the remaining files, and a retried job whose files were all uploaded earlier completes successfully.

## Pipelines

- **Interrupted local file moves also recover on retry**

  The same applied to the copy/move pipeline step: if a move was interrupted after some files had already reached the destination folder, retrying the job reported those files as failed — or failed the whole job when every file had already been moved. Retries now recognize files that already arrived at the destination and complete normally. Moves across drives also copy to a temporary name first, so an interrupted move can no longer leave a half-written file under the final name.

- **A failed conversion no longer leaves a broken file behind**

  Remuxing and the other ffmpeg steps wrote straight to the destination, so a run that failed, was cancelled, or timed out left a truncated file sitting under the final name. The result is now written beside it under a temporary name and moved into place only after ffmpeg finishes cleanly. When the step is set not to overwrite, an existing file at the destination is always kept — including when two jobs happen to finish at the same moment — and a failed run cleans up after itself.

- **Cancelling a step now cancels the run it belongs to**

  Cancelling one step of a multi-step pipeline stopped that step alone. The run itself stayed in Processing with nothing left to finish it, which could hold up everything waiting for that session to complete. Cancelling a step now cancels its whole run and stops the sibling steps with it; a run that had already finished is left alone.

- **Cancelled or timed-out jobs no longer leave tools running in the background**

  When a pipeline job was cancelled or ran past the job timeout, the external tool it had launched — an rclone transfer, ffmpeg processing such as remuxing, transcoding, subtitle burn-in or thumbnails, or danmaku conversion — could keep running in the background even though the job was already marked failed. rclone's temporary file lists could also pile up in the recording folder. Stopping the job now also stops the tool and removes those temporary files.

- **Queued jobs start as soon as a worker is free**

  Pipeline work is handled by two pools of workers that take different kinds of job. Queueing a job woke one worker at random, so a job could sit waiting for the next polling round while a worker that could have run it was parked. Every eligible pool is now woken, and a wakeup that lands while the workers are busy is remembered rather than lost. Two related cases: a fan-out that fails partway now cancels the jobs it had already created instead of leaving them runnable, and a step that can't inspect one of its files fails that file only, instead of abandoning the rest.

- **Deleting a pipeline now updates the job statistics**

  Deleting a pipeline from the Pipeline Jobs page removed the pipeline itself but quietly left its jobs behind, so the Pending, Completed, and Failed counters at the top of the page never went down — deleted failed pipelines kept counting toward the Failed number forever. Deleting a pipeline now also removes its jobs, and the counters reflect it immediately. This also applies to pipelines created before the upgrade, and job records that earlier deletions had already left behind are removed once when you upgrade.

- **A single job can no longer grow its execution log without limit**

  A job that produced a lot of output — for example an rclone step with verbose logging flags in its extra arguments — could keep adding lines to its execution log indefinitely, growing the database until the time-based history cleanup caught up. Each job run now keeps its most recent 5000 log lines and discards the oldest beyond that. Normal runs stay far below the limit, so their logs are unaffected.

## Recording and monitoring

- **Monitoring picks itself back up after a restart**

  A streamer that was in error cooldown when the app was restarted was never given a monitor again, so it stayed unchecked until you toggled monitoring off and on by hand. It now rejoins monitoring at startup, with its first check held until the cooldown expires. Three related cases are fixed with it: a failing live watchdog check no longer retries as fast as the stall timer fires, a streamer you removed can no longer come back from the restart queue, and a short per-request timeout no longer cuts the whole check short before it can finish.

- **Download failures now follow your offline-check setting**

  The number of consecutive download failures before a streamer went into cooldown was fixed at three, regardless of the **Offline check count** you had set. It now uses that same setting, with a floor of two so a single transient failure can't start a cooldown, and the notification you receive names the threshold that actually applied. Cooldown still starts at 60 seconds, doubles with each further failure, and stops at one hour. Offline-check values you had overridden on a site or a template are also kept when those forms are opened or reset, instead of appearing as inherited and being cleared on save.

- **Size and duration limits apply to every stream**

  FLV recordings are split at a keyframe, but on some streams — filtered or encrypted payloads, or an unrecognized codec — no frame was ever recognized as one, so the size and duration limits you set were silently ignored and the recording grew into a single unbounded file. Such a recording is now split once it passes twice the limit, the "split at keyframes only" option is honored when you turn it off, and the size counted against the limit matches what the file on disk actually grows to. Very long recordings also keep a correct timeline instead of eventually folding back to the start.

- **HLS recordings keep data they used to drop**

  Three ways an HLS recording could lose content: the last short piece of a fragmented stream was discarded when the playlist ended instead of being written out; a recording split by size or duration didn't repeat the stream's setup data at the start of the new file, leaving that file unplayable on its own; and a single non-conformant AV1 frame could abort the writer for the whole recording — it is now noted in the log and recording continues. Transport-stream segments whose structure is only partly readable also no longer cause spurious splits.

- **A stream server that comes back can be used again**

  When a stream is offered from several servers, each one that failed was excluded for the rest of the recording. On a long session that meant the app could run out of servers even though one that failed early had since recovered. A server that actually delivers data now clears that history, so the alternatives are available again if it later drops out. A server that fails immediately still counts as a failure, so a stream that never works doesn't cycle forever.

- **Recordings of AV1 and other newer-codec streams keep their codec information**

  When an FLV recording finished, the recorder patched the file's metadata but only understood the original FLV codec numbering. Streams using newer codecs — AV1 video, or Opus audio, for example — had the codec field in the finished file overwritten with a meaningless number, so players and tools that read it reported the wrong codec. The correct codec is now written for both video and audio, older and newer formats alike, and when the recorder can't determine the codec from the stream it keeps what the stream's own metadata declared instead of overwriting it. Streams that carry several video tracks in one feed are recognized as well.

- **A cancelled check no longer blocks a streamer's settings**

  A streamer's settings are worked out once and shared with anything else asking at the same moment. If the request doing that work went away — a check cancelled at the wrong instant — the entry it left behind blocked every later lookup for that streamer, and nothing recovered it. Another waiter now takes over instead.

- **Expired cached stream data is fetched fresh**

  Entries in the download cache that had expired were still handed back as if they were current, and an expired copy on disk could be promoted back into memory. They now count as a miss and the data is fetched again. A disk error while the cache was setting itself up also no longer leaves it marked unusable until the next restart.

## Stability

- **An unexpected internal error no longer ends recordings abruptly**

  Release builds of the server were compiled to terminate the process outright on an internal error, which skipped both the recovery around background tasks and the graceful shutdown that finalizes recordings still in progress. The server now unwinds instead, so such an error is contained and in-flight recordings are closed properly. The standalone `strev` and `mesio` command-line tools keep the smaller, aborting build.

- **Malformed data from a site can't take the app down**

  Several parsers that read data straight off the network — Huya's binary responses, FLV metadata, Bilibili's chat frames, H.264 headers and MPEG-TS tables — could be made to crash, to allocate memory from a size the other side chose, or to lose track of their state on truncated or hostile input. They now turn bad input into an ordinary error and carry on. Valid streams behave exactly as before.

- **An interrupted save no longer wedges the database**

  The app writes to its database through a single connection. A few writes — recording a media output, recording a session segment, deleting a media output — managed their transaction by hand, so a cancelled or failed write could hand that connection back with the transaction still open, and every later write waited on it until the app was restarted. All writes now go through the same transaction handling, which rolls back before the connection is reused.

## Playback

- **Streams that need a login play reliably in the built-in player**

  Live streams whose playlists require cookies or custom headers could stop working after the first request, so playback stalled or failed in the web and desktop player. The player now carries those headers through the whole playlist — quality variants, segments, encryption keys, and low-latency parts — so these streams play consistently.

- **New setting for stream sources on your own network**

  To keep the built-in player's proxy from being pointed at private addresses, it now only reaches public stream sources by default. If you watch or record from a source on your own network — a LAN restreamer, a camera, or a device on your tailnet — turn on **Allow private stream proxy targets** under Network & System settings to permit it.

## Web interface

- **The web interface loads faster**

  The dashboard now downloads less startup code, uses a smaller font file, reuses health checks, and caches built assets for repeat visits. Translation data is also loaded as a reusable compressed file instead of being embedded in every page, the streamers page redraws only the cards that actually changed, and the sessions list fetches what a page needs in a fixed number of database queries instead of a handful per row. These changes keep the existing appearance, animations, language support, and behavior unchanged.

- **A network hiccup no longer signs you out**

  Any failure of the sign-in check — a timeout, or a server that was briefly unreachable — was treated the same as "not signed in", so a momentary blip dropped you back to the login screen. Temporary failures are now retried and your existing session is kept. When a token refresh succeeds but the request that follows it fails, you also get that request's real error instead of the original one.

- **Live updates keep a single connection**

  The download and log streams could end up with more than one connection, or with none at all: a socket closing during cleanup or a sign-in refresh could schedule a reconnect, an old socket's events could overwrite the current one's state, and pausing the log view rebuilt the connection instead of just holding the output. There is now one connection at a time across reconnects, sign-in changes, leaving a page, and pausing.

- **Downloading a log archive no longer strains the page**

  Log archives were assembled in the page's own memory before being handed to you, which a large archive did not survive. The download now goes through the browser directly, so the browser shows its own progress and the page no longer displays a progress bar of its own for it. The server also writes each log straight into the archive instead of holding it in memory first.

- **Clearer settings pages**

  The global settings, notification, recording engine and recording filter pages have been reworked. Related settings sit under headings that say what the group is for, more fields explain themselves when you hover the question mark beside them, and boxes and dropdowns line up with each other. A few settings that were only described in a tooltip now say what they do on the page. The save button no longer sits there greyed out: it appears once you actually change something. None of the settings themselves have changed.

- **Text that stayed in English when using another language**

  In the recording filter editor, the day-of-week tooltips and the day and time preset menus were always shown in English. The priority labels on notification events and channels, and the dashboard's uptime line, were too. They now follow the language you picked.

  Notification events also went by their internal names, so the events list, the type filter and the per-channel event pickers showed entries like "STREAM ONLINE". They now read as ordinary text in your language, and searching the event list still matches either wording.

- **Dropdowns are the height they were meant to be**

  Dropdowns across the pipeline step, engine and preset forms were rendering shorter than the boxes next to them, so rows of settings did not line up. They are now the intended height everywhere.

- **Dropdowns show the saved value when a page opens**

  On several pipeline step, engine and job forms, a dropdown could come up empty or on the wrong entry until you touched it, even though a value was saved. They now show what is stored as soon as the page loads.

- **Theme changes apply immediately again**

  Picking a new theme on the Themes page took effect only after refreshing the page if you had already customized the theme before. Theme presets, colors, and radius changes now apply instantly, as they should.

- **List filters and search stay put when you navigate back**

  On pages like Streamers, Sessions, Pipeline Jobs, Presets, Workflows, and Media Outputs, your search text, filters, sort order, and page position were reset whenever you opened an item and came back, or refreshed the page. They are now kept in the page address, so going back or reloading keeps your place — and you can bookmark or share a filtered view.

- **Pagination buttons now follow your language**

  On paginated lists, the "Previous" and "Next" buttons stayed in English even when the interface was set to another language. They now appear in your selected language.

- **Pipeline execution details display correctly on mobile**

  On narrow screens, the summary cards on a pipeline execution's detail page could overlap their icons and cut off long values like the progress percentage, and the card icons sat at uneven heights. The cards now adapt to smaller screens and their icons line up consistently, so progress, step counts, and start time stay readable.

- **Hover-only actions are reachable from the keyboard**

  The actions that appear when you hover a streamer's row or a log file were invisible if you moved to them with the keyboard instead of the mouse. They now show when they receive focus.

## Security and sign-in

- **Disabled accounts and forced password changes take effect immediately**

  Both were only checked at sign-in, so a user you disabled — or one you required to change their password — kept working with the token they already held until it expired. Both are now checked on every request, together with users who were deleted in the meantime. Signing in to a disabled account also returns a clear "account disabled" result instead of looking like a failed password, and a database error while checking answers with a service error rather than letting the request through.

- **Credentials no longer reach the log files**

  Saving a site's settings or a pipeline preset wrote the whole request and its response into the log, which meant cookies, passwords and processor configuration ended up in the retained log files. Those log lines are gone. Requests whose address carries an access token — the stream proxy, media, downloads and logging endpoints — are now recorded by path only, so the token never lands in a log line either.

- **Signing in no longer holds up other requests**

  Checking a password is deliberately slow work. It was running on the same threads that serve requests, so a few sign-ins at once could stall unrelated requests for as long as they took. It now runs off to the side.

## Desktop app

- **Windows: no more polling while the app is open**

  The desktop app checked every 80 milliseconds whether its window had been minimized, so it could hide to the tray. On Windows it now waits to be told instead, removing that constant background work. macOS and Linux keep the periodic check — their window layer doesn't report the transition — and minimize-to-tray behaves the same on all three.
