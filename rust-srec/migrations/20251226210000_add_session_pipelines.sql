-- Add session-level and paired-segment pipeline definitions to config layers.
-- These store JSON-serialized DagPipelineDefinition (same encoding as existing `pipeline` columns).

ALTER TABLE global_config ADD COLUMN session_complete_pipeline TEXT;
ALTER TABLE global_config ADD COLUMN paired_segment_pipeline TEXT;

ALTER TABLE platform_config ADD COLUMN session_complete_pipeline TEXT;
ALTER TABLE platform_config ADD COLUMN paired_segment_pipeline TEXT;

ALTER TABLE template_config ADD COLUMN session_complete_pipeline TEXT;
ALTER TABLE template_config ADD COLUMN paired_segment_pipeline TEXT;

