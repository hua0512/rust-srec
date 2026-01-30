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
