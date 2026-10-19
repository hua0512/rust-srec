import { useEffect, useState } from 'react';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';

import type { Job } from '@/api/schemas/pipeline';
import type { JobProgressSnapshot } from '@/api/schemas/session';
import { Progress } from '@/components/ui/progress';
import { formatLocalizedDuration } from '@/lib/date-utils';
import { describeJobProgress } from '@/lib/job-progress';

// A running job silent for this long is flagged as making no progress.
const STALL_AFTER_SECS = 60;

/**
 * Whole seconds from `start` to `end`, or undefined while either is unknown.
 * Whole, so a ticking clock never formats as "1m 60s".
 */
function secondsBetween(
  start: string | null | undefined,
  end: string | number | null | undefined,
): number | undefined {
  if (!start || end == null) return undefined;
  const ms = new Date(end).getTime() - Date.parse(start);
  return Math.max(0, Math.round(ms / 1000));
}

/**
 * The job page's Duration and Queue tiles and, while the job runs, its live
 * progress. Holds the one-second clock behind the elapsed time and the stall
 * hint, so its ticks re-render these tiles rather than the whole page.
 */
export function JobProgressPanel({
  job,
  snapshot,
}: {
  job: Job;
  snapshot?: JobProgressSnapshot;
}) {
  const { i18n } = useLingui();
  const running = job.status === 'PROCESSING';
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    if (!running) return;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [running]);

  // The backend records duration and queue wait only for successful runs.
  const elapsedSecs = running
    ? secondsBetween(job.started_at, now)
    : (job.duration_secs ?? secondsBetween(job.started_at, job.completed_at));
  const queueSecs =
    job.queue_wait_secs ?? secondsBetween(job.created_at, job.started_at);

  const { percent, details } = describeJobProgress(
    {
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
    },
    i18n,
  );
  const hasProgress = percent != null || details.length > 0;
  const quietSecs =
    secondsBetween(snapshot?.updated_at ?? job.started_at, now) ?? 0;
  const stalled = quietSecs >= STALL_AFTER_SECS;
  const quietFor = formatLocalizedDuration(quietSecs, i18n.locale);

  return (
    <div className="grid grid-cols-2 gap-4">
      <div className="p-4 rounded-xl bg-background/50 border border-border/50 text-center">
        <div className="text-muted-foreground text-xs font-medium uppercase tracking-wider mb-1">
          <Trans>Duration</Trans>
        </div>
        <div className="text-2xl font-bold tracking-tight">
          {formatLocalizedDuration(elapsedSecs, i18n.locale)}
        </div>
      </div>
      <div className="p-4 rounded-xl bg-background/50 border border-border/50 text-center">
        <div className="text-muted-foreground text-xs font-medium uppercase tracking-wider mb-1">
          <Trans>Queue</Trans>
        </div>
        <div className="text-2xl font-bold tracking-tight text-muted-foreground">
          {formatLocalizedDuration(queueSecs, i18n.locale)}
        </div>
      </div>
      {/* Steps that never report progress, such as commands and deletes,
          drop "Preparing…" after a minute instead of showing it all run. */}
      {running && (hasProgress || !stalled) && (
        <div className="col-span-2 p-4 rounded-xl bg-background/50 border border-border/50">
          <div className="flex items-baseline justify-between mb-2">
            <span className="text-muted-foreground text-xs font-medium uppercase tracking-wider">
              <Trans>Progress</Trans>
            </span>
            {percent != null && (
              <span className="text-sm font-bold font-mono">
                {percent.toFixed(0)}%
              </span>
            )}
          </div>
          {percent != null && <Progress value={percent} />}
          {details.length > 0 && (
            <div className="mt-2 text-xs text-muted-foreground font-mono">
              {details.join(' · ')}
            </div>
          )}
          {!hasProgress && (
            <div className="text-sm text-muted-foreground">
              <Trans>Preparing…</Trans>
            </div>
          )}
          {hasProgress && stalled && (
            <div className="mt-1 text-xs text-muted-foreground/70">
              <Trans>No progress for {quietFor}</Trans>
            </div>
          )}
        </div>
      )}
    </div>
  );
}
