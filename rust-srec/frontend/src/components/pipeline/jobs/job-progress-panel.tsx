import { useLayoutEffect, useState } from 'react';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';

import type { Job } from '@/api/schemas/pipeline';
import type { JobProgressSnapshot } from '@/api/schemas/session';
import { Progress } from '@/components/ui/progress';
import { formatLocalizedDuration } from '@/lib/date-utils';
import { describeJobProgress, snapshotProgress } from '@/lib/job-progress';

// A running job whose progress has not moved for this long is flagged.
const STALL_AFTER_SECS = 60;

/** Seconds from `start` to `end`, or undefined while either is unknown. */
function secondsBetween(
  start: string | null | undefined,
  end: string | number | null | undefined,
): number | undefined {
  if (!start || end == null) return undefined;
  return Math.max(0, (new Date(end).getTime() - Date.parse(start)) / 1000);
}

/**
 * The job page's Duration and Queue tiles and, while the job runs, its live
 * progress. Holds the one-second clock behind the queue wait, the elapsed
 * time and the stall hint, so its ticks re-render these tiles rather than the
 * whole page.
 */
export function JobProgressPanel({
  job,
  snapshot,
}: {
  job: Job;
  snapshot?: JobProgressSnapshot;
}) {
  const { i18n } = useLingui();
  const pending = job.status === 'PENDING';
  const running = job.status === 'PROCESSING';
  const [now, setNow] = useState(Date.now);
  useLayoutEffect(() => {
    if (!pending && !running) return;
    // The clock stands still while the job is neither queued nor running, and
    // its last tick while queued can predate started_at: catch up before paint
    // whenever the job is queued again or starts, so the Queue tile never
    // shows a stale wait and Duration never shows "-" for a running job.
    setNow(Date.now());
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [pending, running]);

  // The backend records duration and queue wait only for successful runs. A
  // job cancelled or failed before it started waited until it ended.
  const elapsedSecs = running
    ? secondsBetween(job.started_at, now)
    : (job.duration_secs ?? secondsBetween(job.started_at, job.completed_at));
  const queueSecs =
    job.queue_wait_secs ??
    secondsBetween(
      job.created_at,
      job.started_at ?? (pending ? now : job.completed_at),
    );

  // A retried job keeps its id, and the progress query keeps its last data
  // through the 404s before the new attempt reports, so a snapshot from
  // before this attempt started belongs to an earlier one.
  const current =
    snapshot &&
    job.started_at &&
    Date.parse(snapshot.updated_at) >= Date.parse(job.started_at)
      ? snapshot
      : undefined;
  const { percent, details } = describeJobProgress(
    snapshotProgress(current),
    i18n,
  );
  const hasProgress = percent != null || details.length > 0;
  const preparing =
    !hasProgress &&
    (secondsBetween(current?.updated_at ?? job.started_at, now) ?? 0) <
      STALL_AFTER_SECS;

  // Timed on this client's clock from the last change in the reported
  // values, not from the report time: rclone repeats an unchanged stats line
  // every second while stuck, and the server's clock may differ.
  const signature = [
    current?.phase,
    current?.percent,
    current?.bytes_done,
    current?.out_time_ms,
    current?.files_done,
  ].join();
  const [lastChange, setLastChange] = useState({ signature, atMs: now });
  if (lastChange.signature !== signature) {
    setLastChange({ signature, atMs: now });
  }
  const quietSecs = (now - lastChange.atMs) / 1000;
  // Finalizing and waiting for another upload leave the reported values
  // alone until they end, however long they take.
  const stalled =
    quietSecs >= STALL_AFTER_SECS &&
    current?.phase !== 'finalizing' &&
    current?.phase !== 'waiting';
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
      {running && (hasProgress || preparing) && (
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
            <div className="mt-2 text-xs text-muted-foreground font-mono wrap-anywhere">
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
