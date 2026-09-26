-- Keep workflow continuation preference indexable without changing FIFO or priority.
-- This is derived data owned by SQLite so all publication, retry, and import paths agree.
ALTER TABLE job ADD COLUMN continuation_rank INTEGER NOT NULL DEFAULT 1
    CHECK (continuation_rank IN (0, 1));

UPDATE job SET continuation_rank = 0
WHERE dag_step_execution_id IN (
    SELECT id FROM dag_step_execution WHERE depends_on_step_ids != '[]'
);

CREATE TRIGGER trg_job_insert_continuation_rank
AFTER INSERT ON job
WHEN NEW.dag_step_execution_id IS NOT NULL
BEGIN
    UPDATE job SET continuation_rank = 0
    WHERE id = NEW.id AND continuation_rank != 0
      AND EXISTS (SELECT 1 FROM dag_step_execution
                  WHERE id = NEW.dag_step_execution_id AND depends_on_step_ids != '[]');
END;

CREATE TRIGGER trg_job_relink_continuation_rank
AFTER UPDATE OF dag_step_execution_id ON job
WHEN OLD.dag_step_execution_id IS NOT NEW.dag_step_execution_id
BEGIN
    UPDATE job SET continuation_rank = COALESCE(
        (SELECT CASE WHEN depends_on_step_ids != '[]' THEN 0 ELSE 1 END
         FROM dag_step_execution WHERE id = NEW.dag_step_execution_id), 1)
    WHERE id = NEW.id;
END;

-- Also cover deferred-FK publication: the job can be inserted before its step.
CREATE TRIGGER trg_step_insert_continuation_rank
AFTER INSERT ON dag_step_execution
BEGIN
    UPDATE job SET continuation_rank = CASE WHEN NEW.depends_on_step_ids != '[]' THEN 0 ELSE 1 END
    WHERE dag_step_execution_id = NEW.id
      AND continuation_rank != CASE WHEN NEW.depends_on_step_ids != '[]' THEN 0 ELSE 1 END;
END;

CREATE TRIGGER trg_step_update_continuation_rank
AFTER UPDATE OF depends_on_step_ids ON dag_step_execution
WHEN OLD.depends_on_step_ids IS NOT NEW.depends_on_step_ids
BEGIN
    UPDATE job SET continuation_rank = CASE WHEN NEW.depends_on_step_ids != '[]' THEN 0 ELSE 1 END
    WHERE dag_step_execution_id = NEW.id
      AND continuation_rank != CASE WHEN NEW.depends_on_step_ids != '[]' THEN 0 ELSE 1 END;
END;

DROP INDEX idx_job_pending_priority_created_at;
DROP INDEX idx_job_pending_type_priority_created_at;
CREATE INDEX idx_job_pending_priority_created_at
    ON job(priority DESC, continuation_rank, created_at, id) WHERE status = 'PENDING';
CREATE INDEX idx_job_pending_type_priority_created_at
    ON job(job_type, priority DESC, continuation_rank, created_at, id) WHERE status = 'PENDING';
