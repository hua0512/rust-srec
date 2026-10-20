import type { I18n } from '@lingui/core';
import { msg } from '@lingui/core/macro';

import type { JobProgressSnapshot } from '@/api/schemas/session';
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

// The store's live progress, or a job page snapshot with plain numbers and nulls.
type JobProgress = {
  [K in Exclude<keyof UploadProgressInput, 'jobId' | 'streamerId'>]?:
    | UploadProgressInput[K]
    | number
    | null;
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
  };
}

/**
 * A job's progress for display: the percent for a bar, capped at 100, and
 * short details in the units the job reports. FFmpeg steps show the output
 * position against the length being written and the speed; jobs counting
 * bytes show bytes and speed. The time left and, in batches, the file in
 * progress follow. No details means the job reports no progress.
 */
export function describeJobProgress(
  progress: JobProgress,
  i18n: I18n,
): { percent?: number; details: string[] } {
  const details: string[] = [];
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
  return {
    percent:
      progress.percent != null ? Math.min(progress.percent, 100) : undefined,
    details,
  };
}
