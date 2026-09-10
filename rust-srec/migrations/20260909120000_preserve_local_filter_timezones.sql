-- Preserve the system-local meaning of existing TimeBased omissions before
-- new runtime defaults become UTC. Cron omissions already mean UTC.
-- CASE guards prevent malformed/non-object legacy JSON from reaching json_type
-- or json_set. Explicit values and unrelated configuration members are retained.
UPDATE filters
SET config = json_set(config, '$.timezone', 'local')
WHERE filter_type = 'TIME_BASED'
  AND CASE WHEN json_valid(config) THEN
      CASE WHEN json_type(config) = 'object' THEN
          json_type(config, '$.timezone') IS NULL
          OR json_type(config, '$.timezone') = 'null'
      ELSE 0 END
  ELSE 0 END;
