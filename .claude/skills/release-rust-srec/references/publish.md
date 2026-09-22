# Publish a rust-srec release

Run commands from the repository root. Use this reference when publication is explicitly authorized; existing authorization remains valid.

Before tagging, ensure the intended release changes are committed on the target revision, the applicable checks in [preparation and validation](prepare.md) have passed, any additional pre-publication checks explicitly required by the user are satisfied, and the tag is not already assigned to a different revision. Unrelated untracked files are not a publication blocker. Do not publish with unresolved version/lockfile or release-content failures.

```sh
git tag rust-srec-vX.Y.Z
git push origin rust-srec-vX.Y.Z
```

Pushing the tag starts `.github/workflows/release-rust-srec.yml`, including its checks, builds, and artifact publication. Those post-push jobs are not a prerequisite for creating the tag. After pushing, inspect the workflow status and report whether it is running, succeeded, or failed.

Push only the intended release ref. If an operation fails or its outcome is uncertain, inspect the local/remote tag state before retrying; do not overwrite an existing release tag. Report the pushed tag and available release/CI status. Do not equate a successful tag push with completed release artifacts while the workflow is still running.
