# FAQ

## Why are `ffmpeg` or other tools not bundled?

One of the common questions is why `rust-srec` does not come bundled with `ffmpeg`, `streamlink`, `yt-dlp`, or other similar tools. There are several reasons for this:

### 1. Licensing and Legal Compliance

`ffmpeg` and many other multimedia tools are often licensed under the GPL (GNU General Public License). Bundling these binaries directly within our distribution could impose certain licensing obligations on `rust-srec` itself or complicate the legal aspects of redistribution. By requiring users to provide their own binaries, we avoid these complexities and respect the licensing of these external projects.

### 2. Binary Size

Tools like `ffmpeg` and `yt-dlp` are quite large. Bundling them would increase the download size of `rust-srec` by tens or even hundreds of megabytes. We prefer to keep our application lightweight and let you manage your own installations of these tools.

### 3. Update Frequency and Independence

External tools, especially `yt-dlp` and `streamlink`, receive frequent updates to maintain compatibility with various streaming platforms. If we bundled them, you would have to wait for a new release of `rust-srec` just to get the latest fixes for your favorite platform. Keeping them separate allows you to update them independently as soon as new versions are available.

### 4. Platform-Specific Customization

Different platforms and hardware configurations may benefit from different builds of `ffmpeg`. For example, some users might need specific hardware acceleration support (like NVENC, QSV, or VAAPI). By having you install your own `ffmpeg`, you can choose the build that best fits your specific needs and environment.

### 5. Separation of Concerns

`rust-srec` is designed to be an orchestrator and manager for stream recording. We focus on providing a robust scheduling and pipeline system, while leveraging well-established, specialized tools for the actual media handling when the built-in Rust engine is not used.

## How do I install these tools?

Please refer to our [Installation Guide](./installation) for instructions on how to install the necessary dependencies for your platform.

## I ran out of disk space. I cleared files, but recordings don't resume. What do I do?

Check the `/health` endpoint or the system health page in the UI. Look at the `output-root` component:

- **Status `Degraded`, `error_kind: not_found`** — you most likely cleared files via a host-side operation (BaoTa file manager, `mv` on the mount source directory, etc.) that broke the Docker bind mount. The container is holding an orphaned inode and needs to be restarted. See the [Docker troubleshooting guide](./docker.md#freeing-up-disk-space-when-using-bind-mounts) for safe cleanup paths.
- **Status `Degraded`, `error_kind: storage_full`** — the disk is genuinely full but the filesystem mount is healthy. Free space via any of the safe cleanup paths in the Docker guide (rust-srec UI, `docker exec`, volume expansion). **No restart needed.** The gate auto-recovers within ~30 seconds of the next attempted download, and every affected streamer cascades out of backoff on the same monitor tick.
- **Status `Degraded`, `error_kind: permission_denied`** — check ownership and mode of the target directory inside the container.
- **Status `Degraded`, `error_kind: read_only`** — the filesystem was remounted read-only. You'll need to remount it read-write.
- **Status `Degraded`, `error_kind: timed_out`** — the filesystem is hung (stale NFS handle, broken bind mount, dead block device). Investigate the underlying storage.
- **Status `Healthy` but recordings still don't resume** — the filesystem is fine from the app's perspective. Check the individual streamer's state in the UI; they may be in backoff from an unrelated error (CDN issues, rate limits). See the logs for the specific `last_error`.

One critical `output_path_inaccessible` notification is also emitted when the gate transitions to `Degraded`, and it contains the same `error_kind` plus a localized recovery hint. See the [notifications doc](../concepts/notifications.md#critical-infrastructure-events).
