# Release Notes

## `unreleased`

## Workflows and file processing

- Fixed rclone uploads trying more times than the preset's Max Retries allowed. rclone retried each failed upload up to 3 times on its own before the next retry started, so a failing upload could run up to three times as often as configured, with no sign of it in the job's progress. Now each try shown in the job's progress is a single upload, and Max Retries limits the total. Presets can still pass `--retries` in their extra arguments to restore rclone's own retries.
- Fixed deleting or renaming a job preset or workflow that a pipeline still uses. Pipelines in the global settings, platforms, templates, streamers and other workflows refer to presets and workflows by name, so deleting or renaming one left those pipelines failing when they ran. Now the change is refused with a message listing where it is still used, and a replace-mode configuration import keeps omitted presets and workflows that the imported pipelines still use, listing them as warnings.
