# Pipeline execution contracts

For contributors implementing processors, persistence, or recovery. User-facing file routing and failure behavior are in the [workflow guide](../concepts/pipeline.md).

## Processor Result Contracts {#processor-result-contracts}

Media processors share output planning, sequential execution and single-file skip
result construction. Unary audio, metadata, thumbnail and remux jobs use the first
output override; their batch jobs require one override per input or none. ASS and
DanmakuFactory instead map strictly against selected video/XML inputs. Naming and
empty-string policies remain processor-specific.

Audio, metadata, thumbnail and remux use staged publication through the shared
driver: every output of a job is written to a temporary file and the whole batch
is published together, so a failure or cancellation part-way through a batch
publishes nothing, and one item's output can never overwrite another item's input.
ASS retains its artifact-matching loop and the same staged publication. Source
deletion remains after successful publication, and skipped sources remain.
Outputs, succeeded/skipped inputs and logs retain their input order and existing
metadata shapes. Batch jobs report the summed input and output sizes of the
items they processed.

A processor that reports some of its inputs as failed fails the job, and a workflow
step with it, even when the remaining inputs were processed. The error names each
failed input. What was published stays on disk and in the job's produced-file
history, so a retry resumes past it. An rclone move or BaiduPCS upload that
fails part-way records each file's own result instead of marking every file
failed. Copy and move steps refuse two inputs that would use the same
destination name, and a thumbnail step takes the first frame of a recording
shorter than the requested timestamp.

Path mechanisms share filesystem resolution while preserving separate policies:
ASS command spelling remains lexical; remux resolves existing relative command
paths and uses best-effort, Windows/macOS case-folded comparisons. Staged output
validation checks native identity (including hard links), resolves nonexistent
leaves through their parents, and propagates I/O errors. Transfer processors still
capture sizes before sources can be consumed and treat only confirmed absence as
an earlier completed transfer.

## Restart recovery of danmu segments {#restart-recovery-of-danmu-segments}

Recovery associates a stored XML output with the stored video segment whose path
has the same name with its extension replaced by `.xml`. The stored paths must
match; recovery does not infer an index from title digits or media-output IDs.
If no segment matches, or different segment indices produce the same XML path,
the XML output is skipped with a warning. Keep original video/XML path records
consistent when restoring a database; unmatched historical files are not
automatically assigned to a segment or replayed through paired processing.

## Error Handling {#error-handling}

Paired-segment and session-complete pipelines receive their video inputs
first, followed by their danmaku inputs. Session-complete inputs are ordered by
segment index; paired inputs retain their collected order. Which danmaku file
belongs to which video is recorded per segment as the pipeline's session
pairing, stored with the pipeline itself and available to every step, including
after a restart or retry. Subtitle conversion and burn-in pair within a segment
only: a segment without a danmaku file leaves its own video without subtitles
and does not shift the pairing of later segments. A subtitle is burned into one
video per job; a second copy of the same recording in the same job is passed
through with a note. No file is written next to the recordings; `_inputs.json` files left by earlier versions are unused and can
be deleted.

Normal completion and restart recovery collect leaf outputs in DAG definition
order, keeping the first occurrence of each path with case folding on Windows
and macOS.
Missing leaf records or malformed output arrays leave recovery incomplete while
preserving the valid outputs. Completing a step twice does not create another
downstream job or increment the DAG's completed-step count again.

Delete, rclone and BaiduPCS retry delays grow exponentially but are capped at
30 seconds per wait, including extreme configured values. Retry counts keep
their configured meaning. FFmpeg progress timestamps exposed as `out_time_ms`
are milliseconds for both supported upstream timestamp keys.

Staged outputs with overwrite disabled use native no-replace publication on
Windows, Linux and macOS, so a filesystem without hard links can still publish
without overwriting a competing file. Systems that support neither no-replace
publication nor hard links return an error. Temporary-file cleanup after an
abandoned processor runs off the async worker and is best effort; a normal
publication waits for its commit or rollback. Subtitle and font paths support
apostrophes and filtergraph delimiters without extra user escaping.

- **Fail-fast**: When a step fails, only the steps that depend on it, directly
  or through other steps, are cancelled. Steps on independent branches keep
  running and finish normally; the workflow stays in progress, with the failure
  recorded as its error, until no step is running, and is then marked failed.
  A retry re-runs only the failed and cancelled steps.
- **Retry**: Failed steps can be retried manually or automatically. A retry
  checks every job it will restart before changing anything; if restarting
  breaks down part-way, the workflow is failed again with the retry error and
  stays retryable. A workflow whose cancelled step never received a job can be
  retried too. At startup, a running step whose job had already failed or been
  cancelled fails its workflow so it can be retried.
- **Logs**: Each step maintains execution logs for debugging

Retries and restart recovery retain the job's earlier step timings, log counters,
file-size metadata and produced-artifact history. Starting another attempt does
not duplicate earlier log entries. The current processor is saved before it runs;
if that write fails or stored execution metadata is invalid, processing stops and
the failure is reported without replacing the original metadata. Completion and
failure updates also preserve additional stored execution-metadata fields.

Produced-artifact history can include files published by earlier attempts. A later
failure does not delete those files. Processors manage their own staged temporary
outputs, with the cleanup limits described above.
