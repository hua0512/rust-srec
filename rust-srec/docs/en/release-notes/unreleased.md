# Release Notes

## `unreleased`

### Fixes

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
