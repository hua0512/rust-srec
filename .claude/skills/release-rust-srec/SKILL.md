---
name: release-rust-srec
description: Prepare a new rust-srec application release end to end — pick the next semver version, bump the workspace version, promote the staged unreleased.md notes into versioned en+zh release-notes pages, reset unreleased, refresh the release-notes index/sidebar/GitHub-body, and emit the rust-srec-vX.Y.Z tag command. Use when asked to cut, prepare, plan, or draft a new rust-srec release or version bump.
---

# Release rust-srec

Prepares a `rust-srec` application release: version bump + localized release notes + GitHub release body, ready for the maintainer to tag. This is a docs-and-version task — it does **not** publish. Tagging/pushing is the maintainer's call; never tag or push unless explicitly asked.

## Source of truth

- **Version**: root `Cargo.toml` `[workspace.package].version`. It flows to `rust-srec/Cargo.toml` and `rust-srec/src-tauri/Cargo.toml` (both `version.workspace = true`) and to Tauri via Cargo fallback. Never hand-edit those downstream files.
- **User-facing notes**: `rust-srec/docs/{en,zh}/release-notes/unreleased.md` — the running draft of changes since the last release. **Promote this**; do not re-derive notes from raw commit subjects.
- **Tag scheme**: `rust-srec-vX.Y.Z` (per-package). Do **not** use bare `vX.Y.Z` — those are legacy tags from an unrelated lineage. `strev`/`mesio` have their own `strev-v*` / `mesio-v*` tags.

## Steps

### 1. Pick the next version
- Last release: `git tag --list 'rust-srec-v*' --sort=-v:refname | head -1`.
- Review changes: `git log --no-merges <last-tag>..HEAD`.
- Semver: fixes/reliability/dependency bumps only → **patch**; a new user-facing feature → **minor**; a breaking change or a migration that needs user action → **major**. rust-srec's 0.3.x line has stayed patch-level for reliability follow-ups.

### 2. Verify the unreleased draft is accurate
- Confirm every item in `unreleased.md` landed *after* the last tag: `git show <last-tag>:rust-srec/docs/en/release-notes/unreleased.md` should be the empty shell ("No staged changes yet…"). See which commits added the items with `git log <last-tag>..HEAD -- rust-srec/docs/en/release-notes/unreleased.md`.
- Never ship an item already announced in the previous `vX.Y.Z.md`.
- Cross-check headline items against real commits (e.g. a "pipeline waits for X" note should map to a `fix(pipeline)` commit touching `rust-srec/src/...`). An item with no backing change since the tag is a red flag — investigate before including it.

### 3. (Optional) Plan and draft with a Workflow
For a thorough pass, run a multi-agent Workflow: one agent categorizes commits and recommends the bump; parallel agents draft the en page, the zh page, and the GitHub body; a final agent adversarially verifies en↔zh consistency and that every claim traces to `unreleased.md` or a commit. Draft agents can write the `vX.Y.Z.md` pages directly (the bump script in step 4 preserves files that already exist).

### 4. Bump version + scaffold docs
```sh
node scripts/bump-rust-srec-version.mjs <X.Y.Z> --docs --dry-run   # preview
node scripts/bump-rust-srec-version.mjs <X.Y.Z> --docs             # apply
```
The script:
- sets `Cargo.toml` `[workspace.package].version`;
- creates `rust-srec/docs/{en,zh}/release-notes/vX.Y.Z.md` **only if absent** (pre-written pages from step 3 are preserved);
- moves the previous "Latest release" entry into "Archive" in both `index.md`;
- inserts `vX.Y.Z` into the VitePress sidebar (`.vitepress/config.mts`) for both locales;
- bumps version pointers in `release-notes.md`;
- rewrites only the *links* in `release-notes-body.md` (its content is replaced in step 8).

### 5. Write the real release notes (en + zh)
Fill `rust-srec/docs/{en,zh}/release-notes/vX.Y.Z.md` by promoting `unreleased.md`:
- open with a 1–2 sentence high-level summary, then themed `##` sections, each bullet shaped `- **Bold lead-in**` + a short explanatory paragraph;
- match the tone/structure of the most recent published page (e.g. `v0.3.1.md`);
- reuse the existing translations in `zh/.../unreleased.md` verbatim — don't re-translate good prose;
- add a `## Compatibility` / `## 兼容性` section only when there's a real migration or behavior note;
- keep en and zh covering the same items in the same order.

### 6. Reset unreleased
Reset both `unreleased.md` to the empty shell:
- en — `# Release Notes` / ``## `unreleased` `` / `No staged changes yet for the next release.`
- zh — `# 更新日志` / ``## `unreleased` `` / `暂无下一个版本的待发布改动。`

### 7. Fix the index placeholders
The script leaves placeholders in both `index.md`; correct them:
- set the **Unreleased** bullet to "no changes staged yet" / "暂无下一个版本的待发布改动";
- replace the auto-generated **Latest** line ("draft release notes…" / "新版本发布说明草稿…") with a real one-line summary;
- delete the extra blank lines the script inserts around the Latest/Archive headings.

### 8. Fill the GitHub release body
Overwrite `rust-srec/docs/release-notes-body.md` with the real body: `## rust-srec vX.Y.Z`, a summary paragraph, `### Highlights`, `### Review before upgrading`, then a closing line linking to `https://docs.srec.rs/en/release-notes/vX.Y.Z` and `/zh/release-notes/vX.Y.Z`. The release CI reads this file directly for the published GitHub Release body. (The bump script only fixed the links here, so a full rewrite is expected.)

### 9. Verify
- Every `vX.Y.Z` link in the sidebars and both `index.md` resolves to an existing file.
- `.vitepress/config.mts` still parses (balanced braces/commas around the inserted sidebar entry).
- If docs deps are installed: `cd rust-srec/docs && pnpm run docs:build`.
- Optional: `cargo build -p rust-srec` to confirm the version bump compiles.

### 10. Tag (only when asked)
```sh
git tag rust-srec-vX.Y.Z
git push origin rust-srec-vX.Y.Z
```

## Files touched
- `Cargo.toml`
- `rust-srec/docs/{en,zh}/release-notes/vX.Y.Z.md` (new)
- `rust-srec/docs/{en,zh}/release-notes/unreleased.md` (reset)
- `rust-srec/docs/{en,zh}/release-notes/index.md`
- `rust-srec/docs/.vitepress/config.mts`
- `rust-srec/docs/release-notes.md`
- `rust-srec/docs/release-notes-body.md`
