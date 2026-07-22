-- Opt-in escape hatch for the stream proxy SSRF guard.
--
-- `validate_target_url` in rust-srec/src/api/routes/stream_proxy.rs rejects
-- proxy targets whose host is localhost, a non-public IP literal, or a
-- hostname resolving to any non-public address. That fails closed for
-- self-hosted setups whose stream sources legitimately live on the local
-- network (a LAN restreamer, a camera behind an internal DNS name, a
-- tailnet address in 100.64.0.0/10).
--
-- When TRUE, the stream proxy skips the private-address checks and the
-- public-only DNS resolver, keeping only the scheme and URL-credential
-- checks. Read per request by `stream_proxy_get`, so toggling it in the
-- global-config UI applies without a restart.
--
-- Defaults to FALSE: only operators who proxy LAN sources should enable
-- it, because it re-opens server-side requests to internal addresses for
-- any authenticated user.

ALTER TABLE global_config
    ADD COLUMN stream_proxy_allow_private_targets BOOLEAN NOT NULL DEFAULT FALSE;
