import type { I18n, MessageDescriptor } from '@lingui/core';
import { msg } from '@lingui/core/macro';

import type { JobPhase, JobProgressSnapshot } from '@/api/schemas/session';
import { getProcessorDefinition } from '@/components/pipeline/presets/processors/registry';
import { formatLocalizedDuration } from '@/lib/date-utils';
import { formatSpeed } from '@/lib/format';
import { formatUploadBytes } from '@/lib/upload-format';
import type { UploadProgressInput } from '@/store/uploads';

/** A media position in milliseconds as "01:02:03". */
export function formatTimecode(ms: number): string {
  const secs = Math.max(0, Math.floor(ms / 1000));
  return [Math.floor(secs / 3600), Math.floor(secs / 60) % 60, secs % 60]
    .map((part) => String(part).padStart(2, '0'))
    .join(':');
}

/** The job type's display name, as the job pages show it. */
export function jobTypeLabel(jobType: string, i18n: I18n): string {
  const definition = getProcessorDefinition(jobType);
  return definition ? i18n._(definition.label) : jobType.replace(/_/g, ' ');
}

// The store's live progress, or a job page snapshot with plain numbers for
// the store's bigints, and nulls.
type JobProgress = {
  [K in Exclude<keyof UploadProgressInput, 'jobId' | 'streamerId'>]?:
    | UploadProgressInput[K]
    | (bigint extends UploadProgressInput[K] ? number : never)
    | null;
};

const PHASE_LABELS: Record<JobPhase, MessageDescriptor> = {
  finalizing: msg`Finalizing...`,
  retrying: msg`Retrying...`,
  // Distinct from "Waiting for a free upload worker.", said of uploads not
  // yet started: this one runs, but BaiduPCS-Go uploads one job at a time.
  waiting: msg`Waiting for another upload to finish`,
};

/** A progress snapshot from the API, or none, as {@link describeJobProgress} takes it. */
export function snapshotProgress(
  snapshot: JobProgressSnapshot | null | undefined,
): JobProgress {
  return {
    percent: snapshot?.percent,
    bytesDone: snapshot?.bytes_done,
    bytesTotal: snapshot?.bytes_total,
    speedBytesPerSec: snapshot?.speed_bytes_per_sec,
    etaSecs: snapshot?.eta_secs,
    outTimeMs: snapshot?.out_time_ms,
    durationMs: snapshot?.duration_ms,
    speedX: snapshot?.speed_x,
    progressFilesDone: snapshot?.files_done,
    progressFilesTotal: snapshot?.files_total,
    phase: snapshot?.phase,
    currentFile: snapshot?.current_file,
    attempt: snapshot?.attempt,
    maxAttempts: snapshot?.max_attempts,
  };
}

/**
 * A job's progress for display: the percent for a bar, capped at 100, and
 * short details in the units the job reports. A phase such as finalizing
 * leads, followed by the uploader's own attempt once it retries. FFmpeg
 * steps show the output position against the length being written and the
 * speed; jobs counting bytes show bytes and speed. The time left, the
 * position in a batch and the name of the file in progress follow; the name
 * comes last so that narrow rows cut it first. No details means the job
 * reports no progress.
 */
export function describeJobProgress(
  progress: JobProgress,
  i18n: I18n,
): { percent?: number; details: string[] } {
  const details: string[] = [];
  if (progress.phase) details.push(i18n._(PHASE_LABELS[progress.phase]));
  const { attempt, maxAttempts: max } = progress;
  if (attempt != null && max != null) {
    // Distinct from the job's own "Attempt n of m": an upload retries
    // within one run of the job.
    details.push(i18n._(msg`Upload attempt ${attempt} of ${max}`));
  }
  const bytes = formatUploadBytes(progress);
  if (progress.outTimeMs != null) {
    const done = formatTimecode(Number(progress.outTimeMs));
    details.push(
      progress.durationMs != null
        ? `${done} / ${formatTimecode(Number(progress.durationMs))}`
        : done,
    );
    if (progress.speedX != null) details.push(`${progress.speedX.toFixed(1)}×`);
  } else if (bytes != null) {
    details.push(bytes);
    if (progress.speedBytesPerSec) {
      details.push(formatSpeed(progress.speedBytesPerSec));
    }
  }
  // Under a second the duration would read in milliseconds.
  if (progress.etaSecs != null && progress.etaSecs >= 1) {
    const eta = formatLocalizedDuration(progress.etaSecs, i18n.locale);
    details.push(i18n._(msg`~${eta} left`));
  }
  const { progressFilesDone: done, progressFilesTotal: total } = progress;
  if (done != null && total != null && total > 1) {
    // Every file can be done while the job is still finishing.
    const index = Math.min(done + 1, total);
    details.push(i18n._(msg`File ${index} of ${total}`));
  }
  if (progress.currentFile) details.push(progress.currentFile);
  return {
    percent:
      progress.percent != null ? Math.min(progress.percent, 100) : undefined,
    details,
  };
}
