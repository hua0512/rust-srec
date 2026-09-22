# Settings reference

Set defaults under **Settings → Global**. Platform, template, and streamer overrides are described in [Configuration layers](../concepts/configuration.md).

## Global Settings {#global-settings}

Access via **Settings** → **Global**. The settings are organized into several categories:

### File Configuration {#file-configuration}
| Setting | Description | Default |
|---------|-------------|---------|
| `record_danmu` | Enable danmaku (live chat) recording | `false` |
| `danmu_statistics` | How chat activity is summarised per session (see below) | defaults |
| `auto_thumbnail` | Automatically generate video thumbnails | `true` |
| `output_folder` | Base directory for recordings (supports templates) | [Deployment default](../getting-started/configuration.md#choose-where-recordings-are-saved); existing databases retain their saved value |
| `output_filename_template` | Filename pattern for recorded files | See [filename templates](./filenames.md) |
| `output_file_format` | Default container format (mp4, flv, etc.) | `flv` |

### Danmu Statistics {#danmu-statistics}

Every recording with `record_danmu` on gets a per-session chat summary: totals, an
activity timeline, the most active chatters, the most frequent words and — where the
platform reports them — gift rankings. `danmu_statistics` tunes that summary, and can
be set globally or overridden per platform, per template and per streamer. Any field
you leave out keeps its default, so `{"top_talkers": 200}` is a complete override.

| Field | Description | Default |
|-------|-------------|---------|
| `enabled` | Compute the summary at all. Turning it off still records the chat files; it only stops the summary, which stores viewer names, from being computed and saved. | `true` |
| `top_talkers` | Chatters and gift senders listed per session (1–500) | `100` |
| `top_words` | Frequent words listed per session (1–500) | `50` |
| `top_gifts` | Gift names listed per session (1–500) | `20` |
| `rate_bucket_secs` | Activity-timeline granularity in seconds. Very long streams are automatically coarsened, so the session page reads the width back rather than assuming it. | `10` |
| `talker_capacity` | Distinct chatters tracked (64–8192). While a stream has fewer than this, counts are exact; above it they become close estimates and the session page marks them with `≈`. | `2048` |
| `word_capacity` | Distinct words tracked (64–8192), same trade-off | `2048` |
| `gift_capacity` | Distinct gift names tracked | `256` |
| `extra_stop_words` | Words to exclude from the frequent-words chart, on top of the built-in list | none |

Out-of-range values are clamped rather than rejected, and a reported list is never
longer than what is tracked.

### Resource Limits {#resource-limits}
| Setting | Description | Default |
|---------|-------------|---------|
| `min_segment_size` | Minimum size before a segment is kept | `1MB` |
| `max_download_duration_secs` | Max duration before splitting the recording | `0` (disabled) |
| `max_part_size` | Max size before splitting the recording | `8GB` |

### Concurrency & Performance {#concurrency-performance}
| Setting | Description | Default |
|---------|-------------|---------|
| `max_concurrent_downloads` | Max simultaneous recording tasks | `6` |
| `max_concurrent_uploads` | Max simultaneous upload tasks | `3` |
| `max_cpu_jobs` | Max concurrent CPU-intensive tasks | `0` (Auto) |
| `max_io_jobs` | Max concurrent I/O-intensive tasks | `8` (0 = Auto) |
| `download_engine` | Engine used for recording (`ffmpeg`, `mesio`, etc.) | `mesio` |
| `queue_freshness_threshold` | When a recording has been waiting for a free slot longer than this, rust-srec re-checks the streamer to refresh stream URLs and headers before starting. Useful on platforms whose signed URLs expire within minutes. Set to `0` to refresh on every queue wait. | `60 Secs` |

Which extractor resolves the stream URL is a separate setting from `download_engine`, is not exposed here, and is set per platform, template, or streamer. See [Engine and extractor selection](./configuration-overrides.md#engine-and-extractor-selection).

### Network & System {#network-system}
| Setting | Description | Default |
|---------|-------------|---------|
| `streamer_check_interval` | Interval between checking streamer status | `60 Secs` |
| `offline_check_interval` | Interval between checking offline status | `20 Secs` |
| `offline_detection_count` | Consecutive offline checks before confirming the streamer is offline. The same resolved count controls when consecutive download failures enter temporary cooldown. Download failures use a minimum threshold of `2`. | `3` |
| `enable_proxy` | Route traffic through an intermediate server | `false` |

### Retention {#retention}

| Setting | Description | Default |
|---------|-------------|---------|
| `job_history_retention_days` | Days to keep terminal pipeline/job and upload history; `0` keeps it indefinitely | `30` |
| `notification_event_log_retention_days` | Days to keep notification events; `0` keeps them indefinitely | `30` |
| `output_retention_days` | Days to keep outputs from ended sessions; `0` disables automatic cleanup | `0` |
| `output_retention_delete_files` | `false`: delete output records only; `true`: delete their tracked local files too | `false` |

Configure output retention under **Global Settings → Retention**. Cleanup runs at startup and every 30 minutes, skips active or recently updated processing and scheduled retries, and defers while processors hold files. File deletion also skips active recording directories and recently modified or shared files; failures keep their records for a later attempt.

**Records only** leaves the physical files on disk. Once their records are removed, changing to **Delete records and files** cannot delete those untracked files later. This policy covers registered media outputs, not arbitrary files, every pipeline derivative, remote uploads, or session segment history.

### Pipeline Configuration {#pipeline-configuration}
Rust-Srec supports custom pipeline steps (e.g., transcripts, notifications, custom scripts) at different stages:
- **Per-segment**: Runs for each recorded segment.
- **Paired Segment**: Runs for video/danmaku pairs.
- **Session Complete**: Runs when the entire recording session ends.

::: info Folder Organization
Set `output_folder` to `{streamer}/%Y-%m-%d` to organize recordings by streamer with date-based subfolders. The `output_filename_template` can then use `%H-%M-%S_{title}` for the filename itself.
:::
