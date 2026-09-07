-- These INTEGER millisecond columns also received SQLx DateTime TEXT binds.
-- Validate all six columns before changing any rows. An unparseable historical
-- value aborts the migration; repair that value and retry startup. SQLite's
-- date parser cannot normalize expanded-year TEXT (for example a date produced
-- by previously interpreting an epoch-ms integer as seconds); this also fails
-- explicitly, so the original value remains available for manual repair.
-- Do not guess a replacement timestamp or reinterpret existing integers as seconds.
CREATE TEMP TABLE _preset_template_timestamp_ms (
    table_name TEXT NOT NULL,
    row_id TEXT NOT NULL,
    column_name TEXT NOT NULL,
    epoch_ms INTEGER NOT NULL CHECK(typeof(epoch_ms) = 'integer'),
    PRIMARY KEY(table_name, row_id, column_name)
);

CREATE TEMP TRIGGER _preset_template_timestamp_invalid
BEFORE INSERT ON _preset_template_timestamp_ms
WHEN NEW.epoch_ms IS NULL
BEGIN
    SELECT RAISE(ABORT, 'Cannot normalize preset/template timestamps: inspect created_at and updated_at in job_presets, pipeline_presets and template_config for invalid date text; repair historical values and retry migration');
END;

WITH historical(table_name, row_id, column_name, value) AS (
    SELECT 'job_presets', id, 'created_at', created_at FROM job_presets
    UNION ALL SELECT 'job_presets', id, 'updated_at', updated_at FROM job_presets
    UNION ALL SELECT 'pipeline_presets', id, 'created_at', created_at FROM pipeline_presets
    UNION ALL SELECT 'pipeline_presets', id, 'updated_at', updated_at FROM pipeline_presets
    UNION ALL SELECT 'template_config', id, 'created_at', created_at FROM template_config
    UNION ALL SELECT 'template_config', id, 'updated_at', updated_at FROM template_config
), parts AS (
    SELECT *,
        -- Remove fractional seconds before SQLite parses the date: its date
        -- functions round fractions, including .999999 across a second boundary.
        CASE WHEN substr(value, 20, 1) = '.'
            THEN substr(value, 1, 19) || ltrim(substr(value, 21), '0123456789')
            ELSE value END AS whole_seconds,
        CASE WHEN substr(value, 20, 1) = '.'
            THEN substr(value, 21, length(substr(value, 21))
                - length(ltrim(substr(value, 21), '0123456789')))
            ELSE '' END AS fraction
    FROM historical
)
INSERT INTO _preset_template_timestamp_ms
SELECT table_name, row_id, column_name,
    CASE
        WHEN typeof(value) = 'integer' THEN value
        WHEN typeof(value) = 'text'
            AND substr(value, 1, 19) GLOB '????-??-??[ T]??:??:??'
            AND (substr(value, 20, 1) <> '.' OR length(fraction) > 0)
            AND strftime('%Y-%m-%dT%H:%M:%S', substr(value, 1, 19), '+0 seconds')
                = replace(substr(value, 1, 19), ' ', 'T')
        THEN unixepoch(whole_seconds) * 1000
            + CAST(substr(fraction || '000', 1, 3) AS INTEGER)
        ELSE NULL
    END
FROM parts;

-- Timestamp normalization is not a user edit. Preserve queued deletions that
-- the ordinary configuration UPDATE triggers cancel, within SQLx's migration
-- transaction, without changing those triggers for subsequent user edits.
CREATE TEMP TABLE _preset_template_retirement_deletions AS
SELECT kind, config_id FROM retirement_config_deletions;

UPDATE job_presets SET
    created_at = (SELECT epoch_ms FROM _preset_template_timestamp_ms
        WHERE table_name = 'job_presets' AND row_id = job_presets.id AND column_name = 'created_at'),
    updated_at = (SELECT epoch_ms FROM _preset_template_timestamp_ms
        WHERE table_name = 'job_presets' AND row_id = job_presets.id AND column_name = 'updated_at');

UPDATE pipeline_presets SET
    created_at = (SELECT epoch_ms FROM _preset_template_timestamp_ms
        WHERE table_name = 'pipeline_presets' AND row_id = pipeline_presets.id AND column_name = 'created_at'),
    updated_at = (SELECT epoch_ms FROM _preset_template_timestamp_ms
        WHERE table_name = 'pipeline_presets' AND row_id = pipeline_presets.id AND column_name = 'updated_at');

UPDATE template_config SET
    created_at = (SELECT epoch_ms FROM _preset_template_timestamp_ms
        WHERE table_name = 'template_config' AND row_id = template_config.id AND column_name = 'created_at'),
    updated_at = (SELECT epoch_ms FROM _preset_template_timestamp_ms
        WHERE table_name = 'template_config' AND row_id = template_config.id AND column_name = 'updated_at');

INSERT INTO retirement_config_deletions (kind, config_id)
SELECT kind, config_id FROM _preset_template_retirement_deletions
WHERE NOT EXISTS (
    SELECT 1 FROM retirement_config_deletions AS pending
    WHERE pending.kind = _preset_template_retirement_deletions.kind
        AND pending.config_id = _preset_template_retirement_deletions.config_id
);

DROP TABLE _preset_template_retirement_deletions;
DROP TABLE _preset_template_timestamp_ms;
