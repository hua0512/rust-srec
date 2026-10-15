-- Output retention inspects nonterminal work independently of retained terminal
-- history. Keep this index proportional to live work and include future statuses.
CREATE INDEX idx_job_nonterminal_id ON job(id)
    WHERE status NOT IN ('COMPLETED', 'FAILED', 'CANCELLED');
