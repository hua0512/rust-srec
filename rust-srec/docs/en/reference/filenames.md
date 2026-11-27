# Filename Template Variables {#filename-template-variables}

Rust-Srec supports two types of placeholders in `output_folder` and `output_filename_template`.

## Curly Brace Variables {#curly-brace-variables}
These are replaced with streamer or session specific metadata.

| Variable | Description |
|----------|-------------|
| `{streamer}` | Streamer display name, with characters invalid in file names replaced by `_` |
| `{title}` | Current stream title, sanitized the same way (capped at 150 bytes in recording templates; not capped in pipeline destination templates) |
| `{platform}` | Platform name (e.g., bilibili) |
| `{session_id}` | Unique ID for the recording session (only in `output_folder`) |

## Percent Placeholders (FFmpeg Style) {#percent-placeholders-ffmpeg-style}
These are replaced with date, time, or sequence information.

| Variable | Description |
|----------|-------------|
| `%Y` | Year (YYYY) |
| `%m` | Month (01-12) |
| `%d` | Day (01-31) |
| `%H` | Hour (00-23) |
| `%M` | Minute (00-59) |
| `%S` | Second (00-59) |
| `%i` | 3-digit zero-padded sequence number for split parts (`001`, defaulting to `1`); only in `output_filename_template` |
| `%t` | Unix timestamp |
| `%%` | Literal percent sign |

Time tokens render in the server's local time zone. Unrecognized `%` sequences are kept as written.

`output_filename_template` names a file, not a path: put directory levels in `output_folder`. Characters that are invalid in file names, such as `/` and `:`, can be replaced with `_`. After expansion, a recording filename stem longer than 200 bytes is truncated at a UTF-8 boundary with a trailing `...` to stay within filesystem limits.

Example: set `output_folder` to `/rec/{streamer}/%Y-%m-%d` and `output_filename_template` to `%H-%M-%S_{title}`.

## Pipeline Destination Placeholders {#pipeline-destination-placeholders}

Pipeline destination fields such as rclone, rship, and Baidu Netdisk `destination_root` and copy/move
`destination` support `{platform}`, `{streamer}`, `{title}`, `{streamer_id}`,
`{session_id}`, and the same `%Y`, `%m`, `%d`, `%H`, `%M`, `%S`, `%t`, and `%%`
time tokens. Time tokens render in the server's local time zone.

Upload processors (rclone, rship, and Baidu Netdisk) expand time tokens with the job creation time by default. Set
`time_anchor` to `session_start` to keep every segment from one live session in
the folder for the session's start date, even when the stream crosses midnight.
Copy/move expands time tokens with the execution time when `time_anchor` is
omitted; set it to `job_created` or `session_start` when deterministic
anchoring is needed.

When anchoring by session start, keep `%Y%m%d-%H%M%S` or `%t` in the filename
template. If multiple sessions send the same basename into one destination
folder, rclone, rship, and filesystem copy/move operations can overwrite or skip files
depending on the operation and arguments.
