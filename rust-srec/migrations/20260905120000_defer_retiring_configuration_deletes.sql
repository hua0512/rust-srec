-- Replace imports retain definitions that retiring recordings may still need.
-- Updating or reimporting a definition cancels its deferred deletion.
CREATE TABLE retirement_config_deletions (
    kind TEXT NOT NULL CHECK (kind IN ('template', 'job_preset', 'pipeline_preset')),
    config_id TEXT NOT NULL,
    PRIMARY KEY (kind, config_id)
);

CREATE TRIGGER trg_template_retirement_update AFTER UPDATE ON template_config
BEGIN
    DELETE FROM retirement_config_deletions WHERE kind = 'template' AND config_id = NEW.id;
END;
CREATE TRIGGER trg_template_retirement_delete AFTER DELETE ON template_config
BEGIN
    DELETE FROM retirement_config_deletions WHERE kind = 'template' AND config_id = OLD.id;
END;
CREATE TRIGGER trg_job_preset_retirement_update AFTER UPDATE ON job_presets
BEGIN
    DELETE FROM retirement_config_deletions WHERE kind = 'job_preset' AND config_id = NEW.id;
END;
CREATE TRIGGER trg_job_preset_retirement_delete AFTER DELETE ON job_presets
BEGIN
    DELETE FROM retirement_config_deletions WHERE kind = 'job_preset' AND config_id = OLD.id;
END;
CREATE TRIGGER trg_pipeline_preset_retirement_update AFTER UPDATE ON pipeline_presets
BEGIN
    DELETE FROM retirement_config_deletions WHERE kind = 'pipeline_preset' AND config_id = NEW.id;
END;
CREATE TRIGGER trg_pipeline_preset_retirement_delete AFTER DELETE ON pipeline_presets
BEGIN
    DELETE FROM retirement_config_deletions WHERE kind = 'pipeline_preset' AND config_id = OLD.id;
END;

-- Explicitly assigning a retained template to an active streamer keeps it.
CREATE TRIGGER trg_streamer_keeps_retired_template_insert AFTER INSERT ON streamers
WHEN NEW.deleted_at IS NULL AND NEW.template_config_id IS NOT NULL
BEGIN
    DELETE FROM retirement_config_deletions WHERE kind = 'template' AND config_id = NEW.template_config_id;
END;
CREATE TRIGGER trg_streamer_keeps_retired_template_update AFTER UPDATE OF template_config_id ON streamers
WHEN NEW.deleted_at IS NULL AND NEW.template_config_id IS NOT NULL
BEGIN
    DELETE FROM retirement_config_deletions WHERE kind = 'template' AND config_id = NEW.template_config_id;
END;
