-- Remove the Bigo `mint_token` option.
--
-- Bigo live checks no longer send an integrity token, so the option has no
-- reader. The web interface's raw JSON editor for platform options checks
-- them against the Bigo options it knows and rejects any other key, so a
-- stored copy is removed from every place Bigo options are kept. Rows whose
-- JSON is malformed are left alone: json_remove would fail on them and stop
-- the upgrade.

-- The Bigo platform's own settings.
UPDATE platform_config
SET platform_specific_config = json_remove(platform_specific_config, '$.mint_token')
WHERE lower(platform_name) = 'bigo'
  AND json_valid(platform_specific_config)
  AND json_type(platform_specific_config, '$.mint_token') IS NOT NULL;

-- Template overrides hold each platform's options under the platform's name,
-- either flat or inside a `platform_specific_config` wrapper; the config
-- resolver reads both.
--
-- Updating a template cancels its pending deletion after a replace import
-- (`trg_template_retirement_update`), so those marks are put back afterwards.
CREATE TEMP TABLE retiring_templates AS
SELECT config_id FROM retirement_config_deletions WHERE kind = 'template';

UPDATE template_config
SET platform_overrides = json_remove(
        platform_overrides,
        '$.bigo.mint_token',
        '$.bigo.platform_specific_config.mint_token'
    )
WHERE json_valid(platform_overrides)
  AND (json_type(platform_overrides, '$.bigo.mint_token') IS NOT NULL
       OR json_type(platform_overrides, '$.bigo.platform_specific_config.mint_token') IS NOT NULL);

INSERT OR IGNORE INTO retirement_config_deletions (kind, config_id)
SELECT 'template', config_id FROM retiring_templates;

DROP TABLE retiring_templates;

-- Streamer overrides hold platform options under `platform_extras`.
UPDATE streamers
SET streamer_specific_config = json_remove(streamer_specific_config, '$.platform_extras.mint_token')
WHERE platform_config_id IN (SELECT id FROM platform_config WHERE lower(platform_name) = 'bigo')
  AND json_valid(streamer_specific_config)
  AND json_type(streamer_specific_config, '$.platform_extras.mint_token') IS NOT NULL;
