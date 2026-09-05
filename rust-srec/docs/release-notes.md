# rust-srec Release Notes

This file is the human-facing release notes guide for the docs workflow.

## GitHub Release body source

- Machine-friendly release body file: [`./release-notes-body.md`](./release-notes-body.md)
- Current release version: `v0.5.1`

The GitHub release workflow now reads `rust-srec/docs/release-notes-body.md` directly.

Update that file before tagging a release if you want the published GitHub Release body to match the curated notes.

## Release workflow

Follow the [application release procedure](../../.claude/skills/release-rust-srec/SKILL.md) for the canonical steps and validation. It distinguishes read-only planning, drafting notes, preparing version/docs changes, and publishing an explicitly authorized release tag. A version-only bump does not promote or reset release notes.

Preparation includes the version/lockfile update, curated en/zh notes, the GitHub body, and index/sidebar checks. Tagging and pushing are a separate publication stage; a request to prepare or draft a release does not authorize it. Reuse existing explicit publication authorization without asking again.

## Docs release pages

- English detailed release notes live under `./en/release-notes/`
- Chinese detailed release notes live under `./zh/release-notes/`
- Archive index pages live at `./en/release-notes/index.md` and `./zh/release-notes/index.md`

## Current docs targets

- Release notes archive: [`/en/release-notes/`](./en/release-notes/index.md)
- English release page: [`/en/release-notes/v0.5.1`](./en/release-notes/v0.5.1.md)
- 中文更新日志归档：[`/zh/release-notes/`](./zh/release-notes/index.md)
- 中文文档页面：[`/zh/release-notes/v0.5.1`](./zh/release-notes/v0.5.1.md)
