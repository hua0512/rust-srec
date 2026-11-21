-- How each notification channel connects. `inherit` follows the global route;
-- `proxy` names a saved proxy, which cannot be deleted while a channel uses it.
-- Email channels always connect directly and keep `inherit`.
ALTER TABLE notification_channel ADD COLUMN proxy_route TEXT NOT NULL DEFAULT 'inherit'
    CHECK (proxy_route IN ('inherit', 'direct', 'system', 'proxy'));
ALTER TABLE notification_channel ADD COLUMN proxy_id TEXT REFERENCES proxies(id) ON DELETE RESTRICT
    CHECK ((proxy_route = 'proxy') = (proxy_id IS NOT NULL));
CREATE INDEX idx_notification_channel_proxy ON notification_channel(proxy_id) WHERE proxy_id IS NOT NULL;
