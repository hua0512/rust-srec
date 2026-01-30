# Security Policy

Security issues are handled with priority.

## Reporting a vulnerability

Please do **not** open a public GitHub issue for suspected security vulnerabilities.

Instead, use GitHub's private vulnerability reporting:

- Go to the repository on GitHub
- Click `Security`
- Click `Report a vulnerability`

Include as much detail as you can:

- A clear description of the issue and potential impact
- Minimal reproduction steps / proof of concept (if applicable)
- Affected components (backend / frontend / desktop / CLI / crates)
- Version/commit information

### Redact secrets

Before sending any logs, configuration, screenshots, or packet captures, remove or mask secrets and sensitive data.
If you are unsure, assume it is sensitive and redact it.

Examples to redact:

- API keys, JWTs, session cookies, OAuth tokens
- Stream keys / ingest URLs
- Passwords, database URLs/credentials
- Access tokens embedded in query params or headers
- Private URLs, IP addresses, user identifiers

If a secret was exposed, revoke/rotate it immediately and mention that you did so in the report.

## Supported versions

Security fixes are applied to the `main` branch and included in the next release.
If you are running an older release, upgrading is the recommended mitigation.

## Disclosure

Please keep reports confidential until a fix is released.
We may request a CVE depending on severity and downstream impact.

## Safe harbor

We welcome good-faith security research and coordinated disclosure.
Do not exfiltrate data, disrupt service, or perform actions that could harm users.
