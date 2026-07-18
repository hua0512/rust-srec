# Release Notes

## `unreleased`

This update rebuilds the **Mesio** engine that records HLS streams so recording is steadier and more reliable, and brings HLS and FLV recording onto the same footing. Encrypted streams record more dependably, memory use stays under control on busy or encrypted streams, and the app no longer downloads the same part of a stream twice when a stream refreshes its access links (as Twitch does). A few engine settings that no longer affected recording were removed, and your existing settings are updated for you.

Recording and repair of FLV and HLS files also uses less CPU, memory, and disk. The app analyzes each stream once instead of over and over, keeps memory in check on very large streams, and finalizes recordings without rewriting the whole file at the end. Newer stream formats and a range of cut-off or damaged inputs are handled more gracefully as well.

Douyu gets a smaller but useful cleanup: you can pick an audio-only stream straight from the quality selector, H.265 (HEVC) streams are detected more accurately, and more "room unavailable" responses are treated as simply offline instead of surfacing as errors.

The dashboard's theme system was rebuilt as well. Dark mode and custom themes now apply before the first frame on both the web and desktop apps — the desktop app no longer flashes white on launch in dark mode — and switching between light and dark is smoother and more predictable.

## Platforms

- **New platform: SOOP**

  You can add SOOP rooms (formerly AfreecaTV, `play.sooplive.co.kr` / `play.sooplive.com`) and record them at your choice of quality. Password-protected rooms and login-required broadcasts are supported through platform settings or cookies. When you set up a SOOP account, the app keeps your sign-in valid automatically so later checks reuse it. Live chat and gifts can be recorded when danmaku is enabled.

- **New platform: Bigo Live**

  You can add Bigo Live rooms (`bigo.tv`) as streamers and record them. Password-protected rooms are supported through platform settings or a `?pwd=` parameter in the room URL, and live chat and gifts can be recorded when danmaku is enabled. By default the app talks to Bigo the same way its website does, for better compatibility. The public stream is a single portrait feed at medium quality (typically around 480p–540p); the higher qualities only available in a browser aren't offered here.

## Desktop app

- **No more white flash when launching in dark mode**

  The main window previously appeared before the saved theme was applied, so dark-mode users saw a white frame on every launch. The theme is now applied before the window is shown, and the loading splash screen follows your chosen theme instead of only the operating system setting — so a light-OS user who prefers a dark app gets a dark splash and a dark first frame.

- **Windows GPU checks no longer flash console windows**

  On Windows systems with NVIDIA GPUs, the desktop app could briefly flash black console windows at startup and every time the background GPU health check ran. GPU checks now run without opening console windows, and startup no longer performs an extra back-to-back check.

## Theme and appearance

- **Custom themes apply at first paint**

  A saved theme preset, imported theme, or color override is now restored before the page first renders, on both web and desktop — no more brief flash of the default palette on load. After updating the app, your saved theme keeps applying immediately unless the app's theme data itself changed in the update.

- **Smoother, more reliable dark/light switching**

  The circular reveal animation no longer occasionally flips the whole screen to the new theme before the animation starts. The header toggle now switches based on the appearance you currently see, so the first click always has an effect when the mode is set to **System**. Selecting a mode that looks identical to the current one (for example switching from **Dark** to **System** while the OS is dark) saves the preference without playing a pointless animation.

- **Theme settings stay in sync across browser tabs**

  Changing the preset, colors, or radius in one tab now updates every other open tab, matching how the light/dark mode already behaved.

- **Delete confirmation buttons show the right text color**

  The text on some delete-confirmation buttons (for deleting a template, notification channel, or workflow) was showing in the wrong color because of a missing style. It now shows in the correct color from your theme.

## Recording timeline

- **HLS discontinuities are labeled accurately**

  The media timeline now tells the difference between the stream signaling a break in continuity — for example when its encoding changes mid-broadcast — and an actual gap where part of the recording is missing. The former is now labeled as a discontinuity rather than a break, the reason a recording was split appears at the point where it happened, and long sessions load their full history instead of only the most recent page.

## Recorded playback

- **Skipping through a recording no longer freezes the video**

  Jumping to a different point while watching a recording — including in fullscreen or maximized view — no longer leaves the video stuck on a black frame. If a jump does get stuck, the player recovers on its own and only shows an error if it still can't continue. Watching live streams is unaffected.

- **Recorded danmaku shows the right times, names, gifts, and super chats**

  Replaying a recording's danmaku now shows the correct times and sender names, and gifts and super chats appear alongside the chat messages. Long chat files now load in the background, so opening the chat for a lengthy recording no longer freezes the page. Danmaku imported from Bilibili XML files still works.

## HLS recording engine

- **Rebuilt HLS engine for more predictable recording**

  The engine that records HLS streams has been rebuilt for steadier, more predictable recording. It handles heavy load more consistently and keeps memory use in check — including the extra work for encrypted streams — so a fast-moving or encrypted stream can no longer make memory climb without limit.

- **More reliable encrypted HLS recording**

  Encrypted streams stay responsive even when a burst of encrypted data arrives, instead of bogging down. For streams delivered in fragmented pieces, the app makes sure the setup data each piece needs is written first, avoiding corrupted output — and if that setup data can't be obtained at all, the recording shows a clear gap instead of stalling.

- **Streams that rotate their access links no longer re-download data**

  When a stream refreshes with new signatures or access tokens in its URLs, parts that were already downloaded are no longer fetched a second time. For streams with known link schemes (for example Twitch signed URLs), the app recognizes the same content across refreshes, and a link that expires mid-download is retried against a fresh one automatically.

- **Missing data shows up as a clear gap**

  When a live stream moves on and drops parts before they can be downloaded, the recording marks a clear gap instead of quietly stalling, so missing data is visible rather than looking like a frozen recording.

## FLV and HLS media processing

- **Lower processing overhead with bounded memory use**

  Analyzing and repairing FLV and HLS recordings now happens once and is reused instead of being repeated, and the app reads only as much of each stream as it needs and works in batches. Memory stays capped while it does so, so a handful of unusually large pieces can't make memory balloon. If a fragmented recording is still waiting on its setup data when that cap is reached, the app starts a fresh output file before continuing, so the recording stays valid.

- **Faster finish for FLV recordings**

  FLV recordings now set aside room for their summary information up front — including audio-only recordings and those without a seek index — and fill it in when recording ends, instead of rewriting the whole file. This cuts disk work and removes a noticeable pause at the end of large recordings. (The old low-latency command-line option is still accepted, but recordings always use the faster in-place method now.)

- **More resilient FLV repair**

  Newer video formats are recognized correctly, the final moments at the end of a file are no longer dropped, empty pieces of data are handled safely, and internal buffering is capped. Together these improve splitting and summary generation for newer streams while keeping damaged or unusual recordings from using unbounded memory during repair.

## Mesio downloader

- **Unified HLS and FLV download sessions**

  HLS and FLV recordings now work the same way under the hood, so progress, retries, and stopping a recording behave consistently no matter which one a stream uses.

- **Simplified Mesio engine settings**

  Several Mesio settings that no longer affected recording were removed from the engine settings form, including the whole **Performance** tab and a few advanced fields that no longer did anything. Your saved settings are updated for you — nothing to do on your end — and the remaining timeout, retry, decryption-key, and gap-skip settings keep working as before.

## Douyu streams

- **Audio-only is part of the quality picker**

  Douyu's quality setting now includes an **Audio only** option alongside the usual quality presets. You can still type a custom Douyu rate when needed, but common choices such as original quality, HD, low quality, and AAC audio are available from the same control.

- **More accurate H.265 detection**

  Douyu streams are now marked as H.265 (HEVC) based on what Douyu's own servers report, and the app requests the matching stream automatically, falling back to the standard one if it isn't available. There's no separate switch to manage, and audio-only requests still use the AAC audio stream.

- **More unavailable-room responses are treated cleanly**

  More of Douyu's "room unavailable" responses are now treated as the room simply being offline. This avoids false errors when a room closes, the streamer goes offline mid-check, or Douyu briefly returns an unavailable response.

## Streamers

- **Filter and sort the streamer list**

  The streamers page has a redesigned filter toolbar for working through large lists. You can filter by assigned template — including streamers with no template — by priority, and by status (for example, showing only streamers currently in an error state), and sort by name, priority, status, or most recently updated. Filtering and sorting apply to your whole list, not just the page you're viewing, so the counts and page numbers stay correct. The toolbar stays compact on desktop and adapts to a mobile layout on smaller screens.

- **Bulk actions for multiple streamers**

  You can now select several streamers and act on them at once. Enter selection mode to pick streamers with the mouse or keyboard, or select the entire current page in one click, then enable, disable, assign a template, set priority (high, normal, or low), or delete the selected streamers in a single action. Each streamer is processed independently: if some fail, the rest still complete, every failure is reported per streamer, and the streamers that failed stay selected so you can retry without starting the selection over.

## Pipeline DAG editor

- **Replace a step without rebuilding its connections**

  Pipeline steps now have a **Replace Step** action in both the list and graph views. Choosing a different job preset or sub-workflow swaps out what that step does while keeping it in the same place, with all its connections to other steps intact.

- **Deleting steps no longer leaves broken dependencies**

  Removing a step now reconnects the steps after it to the steps before it, instead of leaving connections that point at the deleted step. Steps you add after deleting others get their own unique identifiers too, so you don't end up with duplicate steps or ambiguous connections.

- **Long step lists remain accessible**

  Pipeline editors now scroll long lists of steps within the editor instead of cutting off steps below the visible area, and the graph view stays within the same height.

- **Clearer graph view for branching pipelines**

  The pipeline graph now draws steps that split into or merge from several others more clearly: the connecting lines overlap less, cross each other less often, and have bolder arrows, so it's easier to see which step feeds into which. This only changes how the graph looks, not how the pipeline runs.

- **Manually placed steps keep their position**

  Steps you drag into place in the graph now stay where you put them when you add or remove other steps, instead of snapping back on every change. Newly added steps appear next to the steps they connect to without landing on top of others, and the **Auto Layout** button tidies everything back into an automatic arrangement whenever you want it.

## Pipeline uploads

- **Session-start date anchors for upload destinations**

  Rclone pipeline steps now include a `time_anchor` setting. Keep the default `job_created` behavior, or choose `session_start` so date placeholders such as `%Y/%m/%d` use the live session's start time — that way a stream crossing midnight stays in one dated remote folder. Copy/move steps can also use `job_created` or `session_start`, while keeping their current behavior (based on when the step runs) by default.

- **Upload destination dates use server local time**

  Date placeholders in upload destination paths now use the server's local time zone. They previously used UTC, which could send uploads to a different dated folder than the one in the recording's own filename when the server wasn't set to UTC. The right offset is applied for each date, so recordings around daylight-saving changes keep the date their filenames carry, and a retried upload lands in the same dated folder as the original.

## Pipeline step forms

- **Placeholder names show up in help texts again**

  Some pipeline step forms rendered placeholder help text with the names missing — for example the danmaku conversion step showed "Use  and  placeholders." instead of naming `{input}` and `{output}`. The rclone and copy/move destination fields now also document the supported placeholders directly in the form.

- **Clearer error for invalid copy/move step configuration**

  A copy/move step whose saved configuration fails to parse (for example a mistyped option value) now reports the actual parse error instead of a misleading "no destination directory specified" message.

## Recording reliability

- **Recording recovers on its own after repeated download start failures**

  When a stream's server briefly refused downloads (for example returning "not found" errors while the room was still live), three failures in a row would put the streamer into a short cooldown. A timing issue could then leave the room showing as live-with-an-error but with nothing actually recording, until you manually toggled monitoring off and on. Now recording stops cleanly in that case and a fresh check is scheduled for when the cooldown ends, so recording picks back up on its own once the stream is reachable again.

- **Live watchdog restarts downloads that disappeared silently**

  As a safety net, a periodic check now notices when a room has shown as live for five minutes with nothing recording, and starts the recording again instead of ignoring it.

- **Disabling a streamer can no longer be undone by an in-flight status check**

  Disabling a streamer while one of its checks was still running could let that check re-mark it as live and start recording again just after you disabled it. Now, once a disable is saved it always wins — the late check is discarded instead of overriding what you chose.

- **Recordings stay linked to their chat file and timeline**

  Each recording now stays reliably linked to its chat (danmaku) file and its entry on the recording timeline. This holds up on Windows and when recordings are saved to linked or shortcut folders — cases where the chat file or timeline entry could previously fail to line up with the recording.

## Database maintenance

- **Unified, more thorough automatic cleanup**

  The app now cleans up its own database automatically so it doesn't keep growing over time. It clears out old pipeline history, finished and cancelled jobs, expired sign-ins, old notification records, and empty recordings that never captured anything. A quick tidy-up runs when the app starts and every half hour, and any heavier work waits for the maintenance window you've set. Cleanup happens in small batches, so even a large backlog is worked through gradually rather than all at once, and disk space is freed up over time. Anything recent or still in use is always kept.

- **Retention of 0 now means "keep forever"**

  The **Pipeline History Retention** and **Notification Log Retention** settings now treat `0` as "retain indefinitely." Any positive value is the number of days to keep that history; `0` keeps it until you remove it yourself.
