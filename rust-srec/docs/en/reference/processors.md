# Processor reference

Choose processors when editing a workflow or job preset. For a complete example, see [Create a workflow](../concepts/pipeline.md).

## Built-in Processors {#built-in-processors}

Each pipeline step is executed by a specialized processor:

| Processor ID | Function | Core Parameters |
|--------------|----------|-----------------|
| `remux` | Changes container format, optionally re-encoding | `format`, `video_codec`, `audio_codec` |
| `danmaku_factory` | Danmaku conversion | `output_format` (ass) |
| `ass_burnin` | Hard-burn subtitles into video | Processor preset configuration |
| `thumbnail` | Extracts a video frame as an image | `timestamp_secs`, `width`, `quality`, `preserve_resolution` |
| `audio_extract` | Extracts an audio track | `format`, `bitrate`, `sample_rate` |
| `compression` | Bundles files into a ZIP or tar.gz archive | `format`, `compression_level`, `output_path`, `overwrite`, `preserve_paths` |
| `rclone` | Cloud synchronization | `destination_root`, `operation`, `time_anchor`, `args` |
| `baidupcs` | Baidu Netdisk upload via BaiduPCS-Go | `destination_root`, `policy`, `norapid`, `time_anchor`, `args` |
| `copy_move` | Copies or moves local files | Destination and operation settings |
| `metadata` | Writes metadata (nfo, json) | - |
| `delete` | Automatically cleans up files | - |
| `execute` | Runs a program or custom shell command | `program`, `args` or `command`, `scan_output_dir`, `scan_extension` |

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

Fixed Windows native paths accept 8.3 aliases such as `RUNNER~1` and literal brackets. Wildcard executable names and extended `\\?\` namespace paths remain unsupported. Value-bearing Windows commands are checked before launch against cmd's 8,191 UTF-16-unit limit: each inherited binding, the generated command and a conservative expanded command must fit, with 32 units reserved for the launcher. Large JSON arrays may therefore require `program`/`args` or a file data interface.

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
- **Retries**: BaiduPCS-Go's exit code does not reflect upload results, so rust-srec parses its per-file output markers. Retries (in-run and manual job retries) re-send only files without a confirmed result; with the default `skip` policy plus rapid-upload detection, retries avoid uploading files whose results are already confirmed. When some files of a batch fail, the step fails and each file's own result (uploaded, skipped or failed) is recorded. Two inputs with the same file name are rejected before uploading, because every file is uploaded directly to the destination folder. Files that Baidu rejects outright (an illegal name, a file over the size limit, an unreadable file or an exhausted quota) are recorded as failed and are not re-sent by later attempts. A large batch is uploaded through several BaiduPCS-Go invocations so the command line stays within the platform limit, and the login check that precedes an upload is not written to the job log.
- **Limits**: single files above 128 GB are rejected by Baidu, and interrupted transfers restart from the beginning (BaiduPCS-Go v4 no longer supports resume). Upload jobs run one BaiduPCS-Go process at a time because the tool's local state store is single-writer; avoid running the CLI manually against the same config directory while jobs are active.

Logins use the [BaiduPCS-Go v4.0.1 stdin command interface](https://github.com/qjfoidnh/BaiduPCS-Go/blob/v4.0.1/main.go)
with an isolated copy of its standard `pcs_config.json`. This avoids exposing
cookies, BDUSS or STOKEN in process arguments. The CLI's history path is blocked
inside that private directory, so login commands are not saved to history. A
successful result updates only the active account and selected UID in the original
config; rejected logins leave it unchanged. External changes observed when the
original bytes are checked before commit cause the update to be refused. External
CLI/config writers do not share the backend lock and must not run concurrently.

Custom binaries must support the same no-argument command interface, `env`
config-directory report and standard account-config format. Incompatible binaries
return an error; credentials are never retried through command-line flags. Login
subprocesses keep a 60-second deadline, including stdin delivery, with up to five
seconds for forced cleanup. A cancelled request retains the account lock until
cleanup completes. Do not include line breaks or NUL characters in pasted credentials.
