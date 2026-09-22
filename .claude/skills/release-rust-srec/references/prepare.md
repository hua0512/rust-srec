# Prepare and validate a rust-srec release

Paths and shell commands are relative to the repository root. Use this reference for release preparation or a version bump.

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
