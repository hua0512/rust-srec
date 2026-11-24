# Processor reference

Choose processors when editing a workflow or job preset. For a complete example, see [Create a workflow](../concepts/pipeline.md).

## Built-in Processors {#built-in-processors}

Each pipeline step is executed by a specialized processor:

| Processor ID | Function | Core Parameters |
|--------------|----------|-----------------|
| `remux` | Changes container format, optionally re-encoding | `format`, `video_codec`, `audio_codec`, `crf`, `preset`, `hwaccel`, `start_time`, `duration`, `end_time`, `include_recording_metadata`, `remove_input_on_success` |
| `danmaku_factory` | Converts danmaku XML to ASS subtitles with DanmakuFactory | `binary_path`, `args`, `extra_args`, `delete_source_xml_on_success` |
| `ass_burnin` | Hard-burns ASS subtitles into the matching video | `match_strategy`, `video_codec`, `crf`, `preset`, `fonts_dir`, `delete_source_videos_on_success` |
| `thumbnail` | Extracts a video frame as an image | `timestamp_secs`, `width`, `quality`, `preserve_resolution` |
| `audio_extract` | Extracts an audio track | `format`, `bitrate`, `sample_rate`, `channels`, `output_path` |
| `compression` | Bundles files into a ZIP or tar.gz archive | `format`, `compression_level`, `output_path`, `overwrite`, `preserve_paths` |
| `rclone` | Cloud synchronization | `destination_root`, `config_path`, `operation`, `time_anchor`, `max_retries`, `bwlimit`, `bwlimit_file`, `transfers`, `checkers`, `tpslimit`, `tpslimit_burst`, `multi_thread_streams`, `multi_thread_cutoff`, `args`, `proxy_route` |
| `baidupcs` | Baidu Netdisk upload via BaiduPCS-Go | `destination_root`, `binary_path`, `config_dir`, `policy`, `norapid`, `max_retries`, `remove_source_after_upload`, `time_anchor`, `args`, `proxy_route` |
| `copy_move` | Copies or moves local files | `operation`, `destination`, `time_anchor`, `overwrite`, `exclude_patterns` |
| `metadata` | Embeds metadata tags into the media file with FFmpeg | `title`, `artist`, `date`, `album`, `comment`, `custom`, `remove_input_on_success` |
| `delete` | Deletes the files it receives, that is, the outputs of the steps it depends on | `max_retries`, `retry_delay_ms` |
| `execute` | Runs a program or custom shell command | `program`, `args` or `command`, `scan_output_dir`, `scan_extension` |

FFmpeg-based processors use `FFMPEG_PATH` when it is set, and the external-tool processors read their own [path variables](./environment.md#external-tools).

`rclone` and `baidupcs` retry a failed upload within the same job run when the workflow step does not configure its own retry budget; each retry is noted in the job log, and the job's progress and the top bar show which attempt is running. When the workflow step sets `retry.max_attempts` above `1`, each job run performs a single attempt and defers retries to the workflow step's backoff schedule so upload slots are not held during long waits. Rate-limit or quota errors in `rclone` also stop further in-run retries immediately, and permanent errors (such as missing config sections, revoked credentials, or missing paths) fail without retrying.

### Upload proxy {#upload-proxy}

Each `rclone` and `baidupcs` step chooses how it connects under **Proxy** (`proxy_route`): **Inherit** (the default) follows the global **Default proxy**, or the step uses **Direct**, **System proxy** or a [saved proxy](../concepts/configuration.md#saved-proxies). Uploads go to cloud storage rather than a platform, so platform, template and streamer proxy settings do not apply to them. The tools read the proxy from their environment: **Direct** starts them without any proxy variable, **System proxy** passes on the server's, and a saved proxy is passed as `HTTP_PROXY` and `HTTPS_PROXY`, login included. `http`, `https`, `socks5` and `socks5h` proxies all work. The route is resolved when the job starts; a job whose saved proxy has been deleted fails instead of connecting directly. A saved proxy cannot be deleted while a job preset or a pipeline step chooses it, and its password is hidden in job logs and tool output. Logins and the quota check on a BaiduPCS-Go preset's account card follow the Default proxy, and an automatic re-login during an upload uses the step's proxy; commands that only read the local account, such as logging out, use no proxy.

With any choice other than **System proxy**, the tools' own proxy settings must not override it. rclone ignores `RCLONE_HTTP_PROXY`, and a step whose **Extra Arguments** include `--http-proxy` fails. For rclone the step's proxy applies to remotes that connect over HTTP, such as most cloud storage; SFTP, FTP and other remotes that do not use HTTP, and remotes on the local machine, are not routed through it, and use the proxy options of their remote in the rclone config file instead. BaiduPCS-Go prefers the proxy set in its own config (`config set -proxy`), so the step, login or quota check fails until that setting is cleared; the account card still shows the signed-in account and the reason the quota could not be read.

### Execute (`execute`) {#execute-execute}

In the preset editor or a workflow step's configuration dialog, choose **Shell command** or **Program and arguments** under **Execution mode**. Existing shell commands open in shell mode without conversion. Switching modes clears the previous command or program and arguments; the output scan settings are shared and retained.

In program mode, enter the executable in **Program** and use **Add argument** for each argument in order. Each box holds one complete argument, including spaces, quotes, line breaks, or an empty string. Remove a box to omit that argument; leaving it blank passes an empty argument. Saving includes only the selected mode's fields.

Use `program` and `args` to run an executable directly, without a shell:

```json
{
  "program": "ffmpeg",
  "args": ["-nostdin", "-n", "-i", "{input}", "-c", "copy", "/recordings/converted/{streamer}_%Y%m%d_%H%M%S.mp4"],
  "scan_output_dir": "/recordings/converted",
  "scan_extension": "mp4"
}
```

Workflow steps receive no output paths, so `{output}`, `{outputN}` and `{outputs_json}` are empty there. Name the file in the arguments and set `scan_output_dir` (which accepts the same placeholders) so the file is handed to the next step: files that were not in the directory before the command started and were modified after it started are the step's outputs. Without a scan directory the inputs pass through unchanged. The step's size metrics and its list of succeeded inputs cover every input it received and every output it detected, and a scan directory that cannot be read fails the step instead of reporting no outputs.

`program` is a fixed executable name on `PATH` or an executable path; it does not expand placeholders. Each `args` entry is one argument, including an empty string. Arguments support the same file, metadata, JSON-array, and time placeholders as `command`. In paired-segment and session-complete pipelines, `{manifest_json}` expands to the JSON session pairing (which danmu file belongs to which video, per segment); elsewhere it expands to `null`. Inserted values stay literal: quotes, spaces, shell operators, environment-variable references, and further placeholder text are not interpreted. Do not add shell quotes around an argument. The called program still interprets its own options.

Omitting `args` passes no arguments. Use either `program` with optional `args`, or `command`; combining them fails the step. On Windows, `program` rejects `.bat` and `.cmd` files because Windows would run them through a shell. Use `command` for batch scripts. Both modes retain output-directory scanning, pipeline output handling, timeouts, and process cleanup.

Shell syntax follows the server's operating system, regardless of the browser's operating system. For example, `ffmpeg -nostdin -n -i {input} -c copy {input}.mp4` uses a fixed native executable with placeholder arguments and works with the supported template grammar on both platforms, when FFmpeg is installed on the server.

Command templates without recognized placeholders retain their existing shell behavior. With placeholders, the compiler accepts a bounded grammar and passes substituted data through process-local bindings. Placeholders may appear in ordinary argument words or file redirect targets; the command name must be fixed. Unknown and out-of-range placeholders remain literal. A bare empty value contributes no word, while existing quotes retain an empty argument. Empty redirect targets fail before launch.

| Value-bearing templates | Accepted structure |
| --- | --- |
| POSIX | Bare, single-quoted and double-quoted argument fragments; `;`, LF newlines, `&&`, `||`, pipes, `<`, `>`, `>>`, and literal descriptor duplication such as `2>&1` |
| Windows | Fixed native executable names/paths; quoted or bare argument fragments; `&`, `&&`, `||`, file redirects and literal descriptor duplication. Literal-only `echo` and `ver` stages may accompany native commands. |

Templates with placeholders reject command substitution, arithmetic, parameter expansion, here-documents, command groups, assignment prefixes, command-name expansion (including POSIX brace forms) and nested shell wrappers before launch.

Windows additionally rejects pipes, multiline templates, batch scripts, builtin stages with placeholders, literal `%`/`!`/`^` characters in the template, control characters in substituted words, and ambiguous literal quote/backslash combinations. These template-character restrictions apply inside quotes too; put that data in `args` or a placeholder value.

Windows assembles each complete argument before applying the backslash-quote encoding used by common C-style and Shell32 parsers. Programs with custom argument parsers must accept that encoding; follow the called program's argument or data-interface requirements. Redirect filenames use separate rules and cannot contain a double quote.

Windows commands with placeholders require a normal native executable path (wildcards and `\\?\` extended paths are rejected) and must fit within Windows `cmd.exe`'s 8,191-character command-line limit. For large JSON arrays such as `{inputs_json}` or `{manifest_json}`, use `program` and `args` (or pass data via a file) instead.

Use `program` and `args`, or a fixed script with a documented argument/data interface, for unsupported forms. Do not place untrusted values into that program's code or expression arguments: the called program still interprets its own options.

### Archives (`compression`) {#archives-compression}

The `compression` processor bundles its input files into a single archive. It does not re-encode media; the files go in as they are.

| Key | Description | Default |
|-----|-------------|---------|
| `format` | `zip` or `targz` (gzipped tar) | `zip` |
| `compression_level` | `0`–`9`. `0` skips compression — stored entries for `zip`, an uncompressed gzip stream for `targz` — which is the sensible choice for already-compressed video. A value above `9` fails the step rather than being clamped. | `6` |
| `output_path` | Archive to write. When omitted, it is derived from the first input: same directory, same name, with the input's extension replaced by the format's, so `/rec/video.flv` produces `/rec/video.zip`. | derived |
| `overwrite` | Replace an existing archive at that path. With `false`, the step fails instead. | `true` |
| `preserve_paths` | Store each input under its full path inside the archive, minus the leading separator: `/srv/recordings/x/a.mp4` becomes the entry `srv/recordings/x/a.mp4`. The paths are not made relative to a common base. With `false`, every entry is a bare filename at the archive root. | `false` |

The processor accepts a batch, so a step that receives several files from its dependencies produces one archive containing all of them. Its output is the archive path only — the inputs are not deleted, and a `delete` step depending on it removes the archive, not the sources.

Two inputs that would be stored under the same entry name, typically the same file name from two folders with `preserve_paths` off, are rejected before any archive is written. Enable `preserve_paths` or rename one of them.

ZIP entries are always written with ZIP64 sizes, so a single recording larger than 4 GiB archives correctly. The cost is 40 bytes per entry, and the archives still open with ordinary ZIP tools.

### Baidu Netdisk (`baidupcs`) {#baidu-netdisk-baidupcs}

The `baidupcs` processor uploads recordings to Baidu Netdisk through the external [BaiduPCS-Go](https://github.com/qjfoidnh/BaiduPCS-Go) CLI (bundled in the Docker image; install it separately for bare-metal setups and point `BAIDUPCS_PATH` at it if it is not on `PATH`).

- **Login**: open any `baidupcs` preset in the web UI and use the account card to log in with a pasted cookie string (recommended) or BDUSS + STOKEN. Credentials are handed to BaiduPCS-Go and the session persists in its config directory (`BAIDUPCS_GO_CONFIG_DIR`); the same card shows the active account and quota. Enable **Remember for automatic re-login** to also store the credentials server-side (plaintext, like platform cookies): upload jobs then log in again by themselves when the session turns out to be expired — checked before the first attempt and once more before a retry. When a replayed login is rejected (typically because the stored session token was invalidated by a password change), a high-priority `baidupcs_relogin_failed` notification fires and further attempts pause for an hour, so dead credentials produce one alert instead of a failed Baidu call per job. Logging out forgets the stored credentials.
- **Destination**: `destination_root` supports the usual `{streamer}`/`{title}`/time placeholders and always resolves to an absolute Netdisk path. Missing folders are created during upload.
- **Retries**: BaiduPCS-Go's exit code does not reflect upload results, so rust-srec parses its per-file output markers. Retries (in-run, workflow, and manual job retries) carry forward files already confirmed uploaded to the same account and destination so only unconfirmed files are re-sent; with the default `skip` policy plus rapid-upload detection, retries avoid re-uploading already confirmed files. When some files of a batch fail, the step fails and each file's own result (uploaded, skipped or failed) is recorded. Two inputs with the same file name are rejected before uploading, because every file is uploaded directly to the destination folder. Files that Baidu rejects outright (an illegal name, a file over the size limit, an unreadable file or an exhausted quota) are recorded as failed and are not re-sent by later attempts within the same run. When every remaining failed file was rejected permanently, the workflow step fails immediately without spending further retry attempts (if a batch mixes permanent rejections with transient failures, later workflow retries still run the remaining failed files). A large batch is uploaded through several BaiduPCS-Go invocations so the command line stays within the platform limit, and the login check that precedes an upload is not written to the job log.
- **Limits**: single files above 128 GB are rejected by Baidu, and interrupted transfers restart from the beginning (BaiduPCS-Go v4 no longer supports resume). Upload jobs run one BaiduPCS-Go process at a time because the tool's local state store is single-writer; avoid running the CLI manually against the same config directory while jobs are active. Further upload jobs stay queued until the running one finishes, shown as queued uploads in the top bar; their timeout (the step's [`timeout_secs`](./workflows.md#per-step-retries-and-timeouts), otherwise the **IO Job** timeout) starts only when they start running. Baidu Netdisk uploads count toward **Max Uploads** together with rclone uploads, and not toward **Max IO Jobs**. Before sending a file, BaiduPCS-Go reads the whole file to check whether Baidu already has a copy, unless **Disable rapid upload** is on; for a large recording this can take minutes, and the job shows **Preparing…** meanwhile.

- **Login caveats**: credentials are passed over stdin rather than command-line flags and are never written to BaiduPCS-Go's command history; only the active account entry in `pcs_config.json` is updated on success. Do not run external `BaiduPCS-Go` commands against the same config directory concurrently — if the config file changes externally during login, the update is rejected. Custom binaries must be compatible with BaiduPCS-Go v4's interactive stdin command interface and `pcs_config.json` layout. Login attempts time out after 60 seconds, and pasted credentials cannot contain line breaks or NUL characters.
