-- `buffer_size` was stored on every Mesio engine, but it never reached the
-- downloader: FLV recordings always read with Mesio's 64 KiB buffer. It now
-- sets that buffer. The web form and the seeded default engine stored
-- 8388608 (8 MiB) whether or not anyone chose it, so drop exactly that value:
-- those engines keep the 64 KiB buffer they have been recording with instead
-- of allocating 8 MiB per FLV recording. Any other value was set on purpose
-- and is kept.

UPDATE engine_configuration
SET config = json_remove(config, '$.buffer_size')
WHERE engine_type = 'MESIO'
  AND json_valid(config)
  AND json_type(config) = 'object'
  AND json_type(config, '$.buffer_size') = 'integer'
  AND json_extract(config, '$.buffer_size') = 8388608;
