-- Remove danmu statistics checkpoints that belong to ended sessions.
--
-- A checkpoint is read only when a collector resumes a session that is still
-- open. A collector interrupted by a shutdown or crash keeps its checkpoint for
-- that purpose, and if the session then ended while no collector was running,
-- the row stayed until the session itself was deleted. Ending a session
-- deletes its checkpoint, so this one-off pass only has to clear the rows that
-- were stranded before that rule applied.

DELETE FROM danmu_aggregator_state
WHERE session_id IN (SELECT id FROM live_sessions WHERE end_time IS NOT NULL);
