# Support and Versions

Rust-Srec is an open-source project maintained by volunteers.

## Scope and Limits

These are outside what the project provides today. If you need any of them, plan to add and validate it yourself rather than inferring it from feature availability.

- **Deployment** — single-node by design. No clustered scheduling, active-active failover, or managed control plane.
- **Identity** — no SSO, OIDC/SAML, MFA, or SCIM. Tokens carry role names, but the route layer does not enforce a per-role authorization policy; do not treat role labels as an authorization boundary without your own code and deployment review.
- **Audit** — no immutable audit log, SIEM contract, legal-hold workflow, data-classification labels, or automated data-subject-request handling. Sessions, pipeline jobs, notification events, and logs are operational tools, not compliance evidence.
- **Support terms** — no commercial SLA, guaranteed response time, long-term-support branch, or end-of-life calendar.
- **Recovery** — no stated RTO or RPO. Your own [restore drill](./backup-restore.md#restore-drill) measures both.
- **Migrations** — database migrations run at startup and are not reversible.

Security fixes land on `main` and ship in a subsequent release. There are no backports to older tags, so running an older version means running without them.

## Documentation Scope

This documentation set describes the current repository state. The generated Swagger UI on your running backend is authoritative for its API schemas. For older behavior, use the matching [release notes](../release-notes/) and source tag.

| Deployment | Recommended documentation practice |
|---|---|
| Version-tagged release | Pin both images/binaries and retain the matching release notes and OpenAPI JSON |
| `latest` image | Expect it to move when a release is published; do not use where change approval is required |
| `dev` image or `main` branch | Development only; behavior may lead the published documentation |

## Getting Help

- Search existing [GitHub issues](https://github.com/hua0512/rust-srec/issues) before filing a report.
- Use the repository's bug report form for reproducible defects and feature request form for proposals.
- Use [GitHub Discussions](https://github.com/hua0512/rust-srec/discussions) for usage questions when available.
- Report vulnerabilities privately through the [security advisory form](https://github.com/hua0512/rust-srec/security/advisories/new), never in a public issue.

## Diagnostic Bundle

Include enough evidence to reproduce the issue:

- Rust-Srec backend and frontend version or image digest.
- Deployment method, operating system/architecture, and Docker version when relevant.
- Affected platform and a redacted example URL form.
- Exact timestamps with timezone and the expected versus actual outcome.
- Relevant backend/frontend logs and health output.
- Whether the failure affects one streamer, one platform, or all recordings.
- Recent configuration, proxy, storage, or upgrade changes.

Remove JWTs, refresh tokens, session cookies, platform cookies, passwords, webhook URLs, bot tokens, private hostnames/IPs, user identifiers, and private media before submission. Rotate any secret that was exposed.
