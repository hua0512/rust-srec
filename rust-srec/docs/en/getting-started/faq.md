# Frequently Asked Questions

## Which address should I open?

For the Docker files downloaded from this documentation, open `http://localhost:15275`; the API is at `http://localhost:12555/api`. A source development checkout using the example `.env` files uses frontend port `15275` and API port `8080`. See the [installation matrix](./installation.md#choose-a-deployment-method).

## What is the initial account?

Use `admin` / `admin123!`. You will be redirected to change the password, and other protected actions remain blocked until you do. Never expose an installation that still uses the default password.

## Why did adding a streamer not immediately create a file?

Automatic recording begins only when the channel is live, monitoring is enabled, its schedule permits recording, and a download slot is available. Follow [Make Your First Recording](./first-recording.md) to distinguish an offline channel from a failed installation.

## Which channel URL should I use?

Use the direct channel or room URL, not a profile, recorded video, search result, or platform home page. The create form identifies the platform from the URL. See [Supported Platforms](../platforms/) for accepted forms and cookie requirements.

## Do I need FFmpeg or Streamlink?

The built-in Mesio engine does not need either tool. The standard Docker image contains the runtime tools supported by that image. For a binary or source installation, provide `ffmpeg` on `PATH` when selecting FFmpeg features, and provide `streamlink` when selecting Streamlink lookup or download behavior. See [External Tools](./installation.md#external-tools).

## Where are recordings stored?

The standard Docker configuration maps host `./output` to container `/app/output`. Global, platform, template, and streamer settings can override the output path, so the resolved path shown by the application is authoritative. See [Storage and Capacity](../operations/storage.md).

## I freed disk space, but recording did not resume

Open **System Health** and inspect the output-root component:

- `storage_full`: free enough capacity and wait for the next probe; a restart is normally unnecessary.
- `not_found`: a host-side rename or replacement may have broken the bind mount; restart the container after verifying the mount source.
- `permission_denied`: correct ownership and mode on the host path.
- `read_only`: restore the filesystem to read-write operation.
- `timed_out`: investigate a stalled network mount or block device.

Do not rename or replace the bind-mounted root while the container is running. Delete files inside the existing directory, use `docker exec`, or stop the service before changing the mount. See [Docker bind-mount troubleshooting](./docker.md#storage-cleanup).

## A previously working platform stopped recording

Platforms change without notice. Confirm the channel is live, inspect the streamer's check history and logs, refresh required credentials, and test whether the problem affects one channel or the whole platform. Search [GitHub issues](https://github.com/hua0512/rust-srec/issues) before filing a redacted report. Streamlink lookup can be a fallback where the platform page documents it.

## Can I expose Rust-Srec to the Internet?

Only after adding HTTPS, secure cookies, network restrictions, unique secrets, backups, monitoring, and an immediate default-password change. Start with [Production Deployment](../operations/production.md) and [Security](../operations/security.md). Do not publish the backend or Swagger merely because the example Compose file maps a host port.

## How should I upgrade?

Pin the same version for backend and frontend, read intervening release notes, take a consistent backup, and restore that snapshot if rollback is required. See [Upgrade and Rollback](../operations/upgrading.md).

## Is commercial support or an SLA included?

No commercial SLA, guaranteed response time, LTS branch, or end-of-life calendar is published. See [Support and Versions](../operations/support.md) for community channels and the diagnostic information to include.
