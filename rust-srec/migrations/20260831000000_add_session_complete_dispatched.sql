-- Records that a decision has been reached about a session's session-complete
-- pipeline, so `list_ended_sessions_pending_pipeline_recovery` can tell a session
-- whose post-processing was never dispatched from one where it already ran.
--
-- `PipelineManager::run_session_complete_pipeline` sets the flag once
-- `create_dag_pipeline_internal` has durably published the DAG, and also on the
-- empty-pipeline path where there is nothing to publish. Until then the flag stays
-- 0, so a crash or a transient failure between the coordinator emitting
-- `CreateSessionCompleteDag` and the DAG reaching the database leaves the session
-- visible to startup recovery for as long as it takes to restart.
--
-- The DAG's own `segment_source = 'session_complete'` discriminator remains the
-- second half of the check: it covers the narrow window where the DAG committed
-- but this flag had not been written yet.
--
-- Nothing reads the column through `LiveSessionDbModel` — that struct derives
-- `FromRow`, which ignores columns it does not declare, and `create_session` names
-- its columns explicitly — so the default applies to new rows without touching the
-- model.
--
-- The backfill draws a line at upgrade: every session that has already ended is
-- treated as decided. Post-processing genuinely lost before this migration cannot
-- be distinguished from post-processing that completed normally, and re-running it
-- across a streamer's entire recorded history would repeat uploads, transcodes and
-- deletes that already happened.

ALTER TABLE live_sessions
    ADD COLUMN session_complete_dispatched INTEGER NOT NULL DEFAULT 0;

UPDATE live_sessions
SET session_complete_dispatched = 1
WHERE end_time IS NOT NULL;
