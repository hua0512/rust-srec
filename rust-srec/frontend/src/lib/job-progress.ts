import type { I18n } from '@lingui/core';
import { msg } from '@lingui/core/macro';

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

type JobProgress = Omit<UploadProgressInput, 'jobId' | 'streamerId'>;

/**
 * A job's progress for display: the percent for a bar, capped at 100, and
 * short details in the units the job reports. FFmpeg steps show the output
 * position against the length being written, the speed and the time left;
 * jobs counting bytes show bytes and speed; batches add the file in progress.
 * No details means the job reports no progress.
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
    if (progress.etaSecs != null && progress.etaSecs > 0) {
      const eta = formatLocalizedDuration(progress.etaSecs, i18n.locale);
      details.push(i18n._(msg`~${eta} left`));
    }
  } else if (bytes != null) {
    details.push(bytes);
    if (progress.speedBytesPerSec) {
      details.push(formatSpeed(progress.speedBytesPerSec));
    }
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
