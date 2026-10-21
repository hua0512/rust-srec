-- Nothing emits `config_updated` any more and the event catalog no longer
-- offers it, so subscriptions to it route nothing. The match mirrors how
-- subscription names are compared: case-insensitive, ignoring `_`, `.`, `-`
-- and spaces, so the `config.updated` and `ConfigUpdated` spellings go too.

DELETE FROM notification_subscription
WHERE replace(replace(replace(replace(lower(trim(event_name)), '_', ''), '.', ''), '-', ''), ' ', '')
    = 'configupdated';
