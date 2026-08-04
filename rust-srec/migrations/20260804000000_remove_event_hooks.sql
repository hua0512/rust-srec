-- Remove the event_hooks configuration layer.
--
-- `MergedConfig` no longer carries an `event_hooks` field and `ConfigResolver`
-- no longer parses one, so `platform_config.event_hooks` and
-- `template_config.event_hooks` have no reader. Lifecycle side effects are
-- delivered through the channels in `notification::service`.
--
-- ALTER TABLE ... DROP COLUMN applies in place here: the column is a plain
-- nullable TEXT with no index, constraint, or foreign key touching it. A table
-- rebuild is not usable -- `streamers.platform_config_id` and
-- `streamers.template_config_id` are foreign keys into these two tables,
-- `database::init_pool_with_size` opens every connection with
-- `PRAGMA foreign_keys = ON`, and that pragma is a no-op inside the transaction
-- sqlx wraps each migration in, so DROP TABLE on either parent aborts with a
-- foreign key violation while any streamer row exists.

ALTER TABLE platform_config DROP COLUMN event_hooks;
ALTER TABLE template_config DROP COLUMN event_hooks;

-- The streamer layer kept its copy under `$.event_hooks` in the untyped blob
-- read by `MergedConfigBuilder::with_streamer`.
UPDATE streamers
SET streamer_specific_config = json_remove(streamer_specific_config, '$.event_hooks')
WHERE streamer_specific_config IS NOT NULL
  AND json_valid(streamer_specific_config)
  AND json_type(streamer_specific_config, '$.event_hooks') IS NOT NULL;

-- `template_config.platform_overrides` is keyed by platform name, so the key to
-- strip sits one level down under a name that varies per row. json_remove takes
-- a fixed path, so rebuild the object member by member through json_each rather
-- than enumerating the platform names seeded by the initial schema.
UPDATE template_config
SET platform_overrides = (
        SELECT json_group_object(override.key, json_remove(override.value, '$.event_hooks'))
        FROM json_each(template_config.platform_overrides) AS override
    )
WHERE platform_overrides IS NOT NULL
  AND json_valid(platform_overrides)
  AND json_type(platform_overrides) = 'object'
  AND EXISTS (
        SELECT 1
        FROM json_each(template_config.platform_overrides) AS override
        WHERE json_type(override.value) = 'object'
          AND json_type(override.value, '$.event_hooks') IS NOT NULL
    );
