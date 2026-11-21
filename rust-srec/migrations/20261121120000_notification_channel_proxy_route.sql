-- How each notification channel connects. `inherit` follows the global route;
-- `proxy` names a saved proxy, which cannot be deleted while a channel uses it.
-- Only HTTP channels, stored under their canonical type, take another route;
-- email always connects directly and keeps `inherit`.
ALTER TABLE notification_channel ADD COLUMN proxy_route TEXT NOT NULL DEFAULT 'inherit'
    CHECK (proxy_route IN ('inherit', 'direct', 'system', 'proxy'))
    CHECK (proxy_route = 'inherit' OR channel_type IN ('Discord', 'Gotify', 'Telegram', 'Webhook'));
ALTER TABLE notification_channel ADD COLUMN proxy_id TEXT REFERENCES proxies(id) ON DELETE RESTRICT
    CHECK ((proxy_route = 'proxy') = (proxy_id IS NOT NULL));
CREATE INDEX idx_notification_channel_proxy ON notification_channel(proxy_id) WHERE proxy_id IS NOT NULL;
