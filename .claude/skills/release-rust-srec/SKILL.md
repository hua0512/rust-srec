---
name: release-rust-srec
description: Plan, draft notes for, prepare, or publish rust-srec application releases and version bumps. Excludes strev/mesio releases.
---

# Release rust-srec

This is the canonical application release procedure. Paths below are relative to the repository root. Read only the material needed for the requested stage.

## Select the requested outcome

- **Plan**: Recommend a version and summarize release scope, compatibility implications, and unresolved evidence. Use read-only inspection; do not bump versions, reset unreleased notes, commit, or tag.
- **Draft notes**: Write or refine the requested release-note draft. Preserve the workspace version and unreleased staging unless the user also requested release preparation.
- **Prepare**: Follow [preparation and validation](references/prepare.md) to apply the version and release-document changes and deliver the reviewed result. If the request is only a version bump, update the version and lockfile without promoting or resetting notes. Commit/open a PR when that is part of the existing request.
- **Publish**: Complete [preparation](references/prepare.md), then follow [publication](references/publish.md) for the explicitly authorized tag/push operations. Preparing or drafting a release does not by itself authorize publication. Existing explicit publication authorization remains valid; do not ask for it again. If the stage is unclear, perform read-only planning and only the preparation edits already authorized before asking about the dependent step.

## Sources and release scope

- **Version**: root `Cargo.toml` `[workspace.package].version`. Backend and desktop manifests inherit it with `version.workspace = true`; Tauri uses the Cargo fallback. Preserve inheritance instead of adding downstream version literals.
- **Notes**: `rust-srec/docs/{en,zh}/release-notes/unreleased.md` is the curated draft. Promote its relevant items, verifying them against the changes that will actually be released. Commit subjects alone are not a replacement for user-facing notes.
- **Tags**: use `rust-srec-vX.Y.Z`. Bare `vX.Y.Z` tags are from another lineage; `strev-v*` and `mesio-v*` are separate releases.
- **Release body**: `.github/workflows/release-rust-srec.yml` publishes `rust-srec/docs/release-notes-body.md` directly.

List application tags with `git tag --list 'rust-srec-v*' --sort=-v:refname` and select the applicable release baseline for the target branch. Review `git log <last-tag>..HEAD`; inspect relevant changes and the previous versioned notes when needed. A nonempty unreleased page at the last tag or a different commit-message style does not prove an item is invalid. Confirm that claims are present in the target release and have not already been announced; resolve unsupported claims before presenting the notes as ready.

Use an explicitly requested version. Otherwise recommend patch for fixes/reliability/dependency-only changes, minor for new user-facing features, and explain any breaking-change or migration implications using the project's versioning conventions. Do not infer the next version from a historical release-line example.

For a large release, independent drafting/review can be parallelized when delegation is available and authorized. It is optional, with the same evidence and locale checks as a single-agent pass.
