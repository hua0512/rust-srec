# Security

Rust-Srec handles access tokens, platform cookies, notification credentials, recordings, and chat data. Treat the host and every backup as sensitive infrastructure.

## Authentication Boundary

- Change `admin` / `admin123!` immediately. The application enforces a password change for the initial account, but the default credential must still never be exposed to a network.
- Generate unique, random `JWT_SECRET` and `SESSION_SECRET` values of at least 32 characters. Rotating `JWT_SECRET` invalidates access tokens; plan the change as an outage for API clients.
- Access tokens are short-lived. Refresh tokens are rotating session credentials and must be protected like passwords.
- `AUTH_DISABLED=true` is only a local development escape hatch. The backend accepts it only with a loopback bind address; do not build production procedures around it. In that mode requests are refused unless their `Origin` is in the `API_CORS_ORIGINS` allowlist (or matches the request's own `Host`), and unless the `Host` is a loopback name or the configured bind address — the latter is what stops a DNS-rebinding page from reaching the unauthenticated API. Widen the allowlist only for origins you control.
- Repeated failed logins are throttled per account (5 failures per 15 minutes by default) and, much more loosely, per source address; a throttled attempt is answered with `429` and a `Retry-After` delay instead of another password hash. The source address is the TCP peer and `X-Forwarded-For` is not trusted, so behind the bundled frontend or any reverse proxy every login is attributed to the proxy — do not rely on the per-address budget to isolate clients. See [Configuration](../getting-started/configuration.md#login-throttling).
- Tokens carry role names and exports include user accounts, but the route layer does not enforce a per-role authorization policy, and there is no identity-provider integration. See [Scope and Limits](./support.md#scope-and-limits).

## Network and Session Security

Terminate TLS before the frontend, set `COOKIE_SECURE=true` only if the proxy does not send `X-Forwarded-Proto: https`, and restrict both host ports with a firewall. Keep the backend private unless direct API consumers need it. Do not expose Swagger, media proxy routes, or logs to anonymous Internet clients.

The stream proxy blocks private-network targets by default. Enabling `stream_proxy_allow_private_targets` allows authenticated users to make the service fetch internal addresses; enable it only for an explicit LAN-camera or restream use case.

## Secrets and Files

- Restrict `.env`, `DATA_DIR`, `CONFIG_DIR`, `LOG_DIR`, configuration exports, and backup media.
- Platform cookies and passwords can be present in configuration exports. Notification channels can contain SMTP passwords, bot tokens, webhook secrets, and custom headers.
- The Baidu Netdisk login session lives in BaiduPCS-Go's config directory (`BAIDUPCS_GO_CONFIG_DIR`, `/app/config/BaiduPCS-Go` in Docker) — protect and back it up like a cookie store. With **Remember for automatic re-login** enabled, the login material is additionally stored plaintext in the application database (`tool_credentials`), like platform cookies; logging out deletes it. During a login (including automatic re-login) the credentials briefly appear on the BaiduPCS-Go process command line, which other local processes can observe; treat host shell access as equivalent to account access.
- Redact tokens, private URLs, usernames, cookies, and filesystem paths before sharing logs or screenshots.
- Run containers without unnecessary host mounts or device access. Add GPU devices only when the selected pipeline requires them.
- Review any `execute`, upload, move, or delete-source pipeline step as code with filesystem and network access.

## Vulnerability Handling

Do not report suspected vulnerabilities in a public issue. Use [GitHub private vulnerability reporting](https://github.com/hua0512/rust-srec/security/advisories/new) and include the affected version, impact, and a minimal reproduction with secrets removed. Security fixes target `main` and ship in a subsequent release.

For operational evidence and limitations, also read [Data Governance](./data-governance.md) and [Support and Versions](./support.md).
