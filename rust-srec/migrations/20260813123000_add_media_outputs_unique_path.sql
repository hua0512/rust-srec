-- JobQueue::materialize_upload_outputs registers uploaded artifacts with a
-- read-check-insert; without a uniqueness guarantee, concurrent upload jobs
-- (e.g. rclone and baidupcs fan-out branches finishing together) could each
-- observe the row as absent and insert duplicates. Deduplicate first, keeping
-- the oldest row per key so media ids already referenced by clients stay
-- valid, then enforce one row per (session_id, file_path).
DELETE FROM media_outputs
WHERE rowid NOT IN (
    SELECT MIN(rowid) FROM media_outputs GROUP BY session_id, file_path
);

CREATE UNIQUE INDEX idx_media_outputs_session_path
    ON media_outputs (session_id, file_path);
