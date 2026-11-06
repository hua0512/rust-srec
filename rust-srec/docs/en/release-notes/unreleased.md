# Release Notes

## `unreleased`

## Workflows and file processing

- Fixed rclone uploads trying more times than the preset's Max Retries allowed. rclone retried each failed upload up to 3 times on its own before the next retry started, so a failing upload could run up to three times as often as configured, with no sign of it in the job's progress. Now each try shown in the job's progress is a single upload, and Max Retries limits the total. Presets can still pass `--retries` in their extra arguments to restore rclone's own retries.
