# Filename Template Variables {#filename-template-variables}

Rust-Srec supports two types of placeholders in `output_folder` and `output_filename_template`.

## Curly Brace Variables {#curly-brace-variables}
These are replaced with streamer or session specific metadata.

| Variable | Description |
|----------|-------------|
| `{streamer}` | Streamer display name |
| `{title}` | Current stream title |
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
| `%i` | Sequence number for split parts |
| `%t` | Unix timestamp |
| `%%` | Literal percent sign |

Example: `{streamer}/%Y-%m-%d/%H-%M-%S_{title}`

## Pipeline Destination Placeholders {#pipeline-destination-placeholders}

Pipeline destination fields such as rclone `destination_root` and copy/move
`destination` support `{platform}`, `{streamer}`, `{title}`, `{streamer_id}`,
`{session_id}`, and the same `%Y`, `%m`, `%d`, `%H`, `%M`, `%S`, `%t`, and `%%`
time tokens. Time tokens render in the server's local time zone.

Rclone expands time tokens with the job creation time by default. Set
`time_anchor` to `session_start` to keep every segment from one live session in
the folder for the session's start date, even when the stream crosses midnight.
Copy/move preserves its historical execution-time expansion when `time_anchor`
is omitted; set it to `job_created` or `session_start` when deterministic
anchoring is needed.

When anchoring by session start, keep `%Y%m%d-%H%M%S` or `%t` in the filename
template. If multiple sessions send the same basename into one destination
folder, rclone and filesystem copy/move operations can overwrite or skip files
depending on the operation and arguments.
