# Release Notes

## `unreleased`

## Workflows and file processing

- Fixed rclone uploads trying more times than the preset's Max Retries allowed. rclone retried each failed upload up to 3 times on its own before the next retry started, so a failing upload could run up to three times as often as configured, with no sign of it in the job's progress. Now each try shown in the job's progress is a single upload, and Max Retries limits the total. Presets can still pass `--retries` in their extra arguments to restore rclone's own retries.
- Fixed deleting or renaming a job preset or workflow that a pipeline still uses. Pipelines in the global settings, platforms, templates, streamers and other workflows refer to presets and workflows by name, so deleting or renaming one left those pipelines failing when they ran. Now the change is refused and a dialog lists where it is still used, with a link to each. A replace-mode configuration import also keeps omitted presets and workflows that the imported pipelines still use, listing them as warnings.

## Web interface

- Template cards now show how many streamers use each template under **Used by**, with a link to those streamers. Deleting a template that streamers still use now explains why it can't be deleted and offers **Show streamers**, instead of showing a raw server error.
- Fixed the web interface not recognising several server responses and showing a generic error or the server's raw message instead. Sessions without danmu statistics showed a load error with **Retry** instead of saying the statistics are not available, and refreshing an expired stream in the player failed instead of fetching the stream again. A proxy name that is already taken, a proxy with the same address and username, a proxy changed elsewhere, and a site that already has an account profile showed the server's message instead of an explanation or the list of settings that still use them. Cancelling a pipeline that had just finished reported an error, a session whose password must be changed was only sent to the change-password page on the next page load, and the sign-in error shown when the server could not be reached was not translated. The desktop app was not affected.
- Fixed the streamer list returning to the first page after saving a streamer, using the back arrow on its edit page, or cancelling **Add a streamer**. The list now reopens on the page you left, with the same search, filters and sorting.
- Fixed the **Per page** picker disappearing from lists such as streamers, pipeline jobs and job presets once everything fit on one page. Choosing a larger page size left no way to switch back to a smaller one. The picker now stays visible while a smaller size would split the list into pages again.
