# rust-srec Release Notes

This file is the human-facing release notes guide for the docs workflow.

## GitHub Release body source

- Machine-friendly release body file: [`./release-notes-body.md`](./release-notes-body.md)
- Current release version: `v0.3.2`

The GitHub release workflow now reads `rust-srec/docs/release-notes-body.md` directly.

Update that file before tagging a release if you want the published GitHub Release body to match the curated notes.

## Suggested release workflow

1. Run `node scripts/bump-rust-srec-version.mjs <X.Y.Z> --docs`
2. If you want the new localized pages to start from the latest version's structure, add `--from-latest`
3. Fill in:
   - `./release-notes-body.md`
   - `./en/release-notes/vX.Y.Z.md`
   - `./zh/release-notes/vX.Y.Z.md`
4. Review archive links and sidebar entries
5. Tag the release as `rust-srec-vX.Y.Z`

## Docs release pages

- English detailed release notes live under `./en/release-notes/`
- Chinese detailed release notes live under `./zh/release-notes/`
- Archive index pages live at `./en/release-notes/index.md` and `./zh/release-notes/index.md`

## Current docs targets

- Release notes archive: [`/en/release-notes/`](./en/release-notes/index.md)
- English release page: [`/en/release-notes/v0.3.2`](./en/release-notes/v0.3.2.md)
- 中文更新日志归档：[`/zh/release-notes/`](./zh/release-notes/index.md)
- 中文文档页面：[`/zh/release-notes/v0.3.2`](./zh/release-notes/v0.3.2.md)
