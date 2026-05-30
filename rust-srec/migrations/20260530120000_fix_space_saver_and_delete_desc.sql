-- Fix the built-in "Space Saver" pipeline and the "delete_source" preset description.
--
-- A standalone `delete` step in a DAG receives the merged OUTPUTS of the steps it
-- depends_on (see complete_step_and_check_dependents / merge_dependency_outputs), never
-- the original inputs those steps consumed. The remux/transcode processor returns only its
-- newly produced file as output, so a `delete` step placed after it deletes the converted
-- result, not the source recording.
--
-- "Space Saver" was wired as compress -> delete_source(depends_on: compress), so it deleted
-- the compressed file and kept the original -- the opposite of its description. The correct
-- idiom (already used by remux_clean / Stream Archive) is to delete the source inside the
-- transcode step via remove_input_on_success, which knows which path is the input.

-- Rewrite Space Saver to a single compress step that removes its own input on success.
-- Guard on the presence of the standalone delete_source step so installs that have already
-- customized this preset are left untouched.
UPDATE pipeline_presets
SET dag_definition = '{"name":"Space Saver","steps":[{"id":"compress","step":{"type":"inline","processor":"remux","config":{"video_codec":"h265","audio_codec":"aac","audio_bitrate":"96k","preset":"slow","crf":28,"format":"mp4","faststart":true,"overwrite":true,"remove_input_on_success":true}},"depends_on":[]}]}',
    updated_at = unixepoch('now') * 1000
WHERE id = 'pipeline-space-saver'
  AND dag_definition LIKE '%delete_source%';

-- Correct the delete_source preset description so it no longer recommends placing the step
-- after a transcode. Guard on the original seeded text so user-edited descriptions are kept.
UPDATE job_presets
SET description = 'Deletes the files produced by the previous step. Safe after an Upload step (removes the uploaded local copy). After a transcode/remux it deletes the converted result, not the original recording -- enable "Remove Input on Success" on the transcode step to delete the source instead.',
    updated_at = unixepoch('now') * 1000
WHERE id = 'preset-default-delete'
  AND description = 'Delete the source file. Use as the last step in a pipeline to clean up.';
