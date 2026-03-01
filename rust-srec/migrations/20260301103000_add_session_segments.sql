CREATE TABLE session_segments (
    id TEXT PRIMARY KEY NOT NULL,
    session_id TEXT NOT NULL,
    segment_index INTEGER NOT NULL,
    file_path TEXT NOT NULL,
    duration_secs REAL NOT NULL,
    size_bytes INTEGER NOT NULL,
    split_reason_code TEXT,
    split_reason_details_json TEXT,
    created_at INTEGER NOT NULL,
    FOREIGN KEY (session_id) REFERENCES live_sessions(id) ON DELETE CASCADE
);

CREATE INDEX idx_session_segments_session_id_created_at
    ON session_segments (session_id, created_at);

CREATE INDEX idx_session_segments_session_id_segment_index
    ON session_segments (session_id, segment_index);

CREATE INDEX idx_session_segments_session_id_file_path
    ON session_segments (session_id, file_path);

CREATE INDEX idx_session_segments_split_reason_code
    ON session_segments(split_reason_code)
    WHERE split_reason_code IS NOT NULL;
