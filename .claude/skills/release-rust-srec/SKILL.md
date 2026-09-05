---
name: release-rust-srec
description: Plan, prepare, or publish a rust-srec application release, including workspace version changes and localized release notes. Select the stage from the user's request; planning does not modify release files. Use for application release/version tasks, not ordinary development or strev/mesio releases.
---

# Release rust-srec

This is the canonical application release procedure. Paths below are relative to the repository root. Read only the material needed for the requested stage.

## Select the requested outcome

- **Plan**: Recommend a version and summarize release scope, compatibility implications, and unresolved evidence. Use read-only inspection; do not bump versions, reset unreleased notes, commit, or tag.
- **Draft notes**: Write or refine the requested release-note draft. Preserve the workspace version and unreleased staging unless the user also requested release preparation.
- **Prepare**: Apply the version and release-document changes, validate them, and deliver the reviewed result. If the request is only a version bump, update the version and lockfile without promoting or resetting notes. Commit/open a PR when that is part of the existing request.
- **Publish**: Complete preparation, then perform the explicitly authorized tag/push operations. Preparing or drafting a release does not by itself authorize publication. Existing explicit publication authorization remains valid; do not ask for it again. If the stage is unclear, perform read-only planning and only the preparation edits already authorized before asking about the dependent step.

## Sources and release scope

- **Version**: root `Cargo.toml` `[workspace.package].version`. Backend and desktop manifests inherit it with `version.workspace = true`; Tauri uses the Cargo fallback. Preserve inheritance instead of adding downstream version literals.
- **Notes**: `rust-srec/docs/{en,zh}/release-notes/unreleased.md` is the curated draft. Promote its relevant items, verifying them against the changes that will actually be released. Commit subjects alone are not a replacement for user-facing notes.
- **Tags**: use `rust-srec-vX.Y.Z`. Bare `vX.Y.Z` tags are from another lineage; `strev-v*` and `mesio-v*` are separate releases.
- **Release body**: `.github/workflows/release-rust-srec.yml` publishes `rust-srec/docs/release-notes-body.md` directly.

List application tags with `git tag --list 'rust-srec-v*' --sort=-v:refname` and select the applicable release baseline for the target branch. Review `git log <last-tag>..HEAD`; inspect relevant changes and the previous versioned notes when needed. A nonempty unreleased page at the last tag or a different commit-message style does not prove an item is invalid. Confirm that claims are present in the target release and have not already been announced; resolve unsupported claims before presenting the notes as ready.

Use an explicitly requested version. Otherwise recommend patch for fixes/reliability/dependency-only changes, minor for new user-facing features, and explain any breaking-change or migration implications using the project's versioning conventions. Do not infer the next version from a historical release-line example.

For a large release, independent drafting/review can be parallelized when delegation is available and authorized. It is optional, with the same evidence and locale checks as a single-agent pass.

## Prepare the files

For full release preparation, preview and apply the existing helper:

```sh
node scripts/bump-rust-srec-version.mjs <X.Y.Z> --docs --dry-run
node scripts/bump-rust-srec-version.mjs <X.Y.Z> --docs
```

Omit `--docs` for a version-only request. The helper updates the workspace version and runs `cargo update --workspace` to synchronize `Cargo.lock`. If the lockfile update fails, resolve it or report preparation as incomplete; the script's final output alone is not proof of success. Review the lockfile diff for unrelated changes. If bumping without the helper, synchronize the lockfile explicitly.

With `--docs`, the helper creates versioned en/zh pages only if absent, updates the Latest/Archive indexes, inserts sidebar links, and updates version pointers in `release-notes.md` and links in `release-notes-body.md`. Existing versioned pages are preserved. Use `--from-latest` only when a previous page's structure is a useful scaffold; it does not supply this release's content.

For full preparation:

1. Promote the selected unreleased items to `rust-srec/docs/{en,zh}/release-notes/vX.Y.Z.md`. Follow the most recent published page's structure, reuse good existing translations, and keep both languages covering the same items in the same order. Include compatibility guidance when there is a real behavior change or migration.
2. After preserving the promoted content, reset the corresponding unreleased items. If all staged items are included, use the empty shells: en `# Release Notes` / ``## `unreleased` `` / `No staged changes yet for the next release.`; zh `# 更新日志` / ``## `unreleased` `` / `暂无下一个版本的待发布改动。`. Preserve any explicitly deferred items.
3. Replace the helper's draft placeholders in both `index.md` files with an accurate Latest summary and the remaining Unreleased status. Remove incidental extra blank lines introduced by the helper.
4. Fill `rust-srec/docs/release-notes-body.md` with `## rust-srec vX.Y.Z`, a short summary, highlights, any actual upgrade guidance, and links to `https://docs.srec.rs/en/release-notes/vX.Y.Z` and the matching zh page. Check that it agrees with the detailed notes.

## Validate and finish preparation

- Check versioned pages, index/sidebar links, and version pointers for consistency. Verify en/zh content parity and the evidence behind release claims.
- Run `cargo metadata --locked --format-version 1` to verify lockfile consistency without compilation. Confirm the backend and desktop workspace versions match the requested version.
- For release-document changes, run `pnpm -C rust-srec/docs run docs:build` to check configuration and links. Restore dependencies with the pinned toolchain if feasible; otherwise report this check as outstanding and its impact. A version-only request does not need a docs build.
- A pure version/docs change does not require a backend compilation or full workspace test run locally. Expand validation for actual dependency/build changes or explicit release requirements using the root task-based guidance and release CI.
- Review the final diff. Preparation is complete when the requested files are consistent and relevant validation is accounted for; distinguish completed preparation from missing evidence needed before publication. Commit or create a PR if already requested. Otherwise provide the result and intended tag without waiting for permission to complete ordinary preparation.

## Publish when authorized

Before tagging, ensure the intended release changes are committed on the target revision, the required release checks have passed, and the tag is not already assigned to a different revision. Unrelated untracked files are not a publication blocker. Do not publish with unresolved version/lockfile or release-content failures.

```sh
git tag rust-srec-vX.Y.Z
git push origin rust-srec-vX.Y.Z
```

Push only the intended release ref. If an operation fails or its outcome is uncertain, inspect the local/remote tag state before retrying; do not overwrite an existing release tag. Report the pushed tag and available release/CI status. Do not equate a successful tag push with completed release artifacts while the workflow is still running.
