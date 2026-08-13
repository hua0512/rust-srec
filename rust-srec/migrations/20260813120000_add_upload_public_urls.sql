-- Public HTTP URLs for uploaded files (rclone `public_url_mode`), so session
-- previews can fall back to the remote copy after local files are deleted.

-- Per-upload-result URL, written alongside remote_path by
-- JobQueue::persist_upload_records.
ALTER TABLE upload_records ADD COLUMN public_url TEXT;

-- Durable copy on the session file row itself: upload_records are pruned by
-- the job-history retention sweep, while media_outputs rows live as long as
-- the session. GET /api/media/{id}/content redirects here when the local
-- file no longer exists.
ALTER TABLE media_outputs ADD COLUMN remote_url TEXT;
