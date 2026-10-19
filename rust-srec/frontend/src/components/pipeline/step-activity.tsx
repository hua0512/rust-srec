import type { I18n } from '@lingui/core';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import type { ReactNode } from 'react';

import type { DagStep } from '@/api/schemas';
import { RetryCountdown } from '@/components/pipeline/retry-countdown';
import { stepDisplayStatus } from '@/components/pipeline/status-config';
import { useNow } from '@/hooks/use-now';
import { formatLocalizedDuration } from '@/lib/date-utils';
import { describeJobProgress, snapshotProgress } from '@/lib/job-progress';

/** A step's progress for display; only a running step reports any. */
export function stepProgress(
  step: DagStep,
  i18n: I18n,
): { percent?: number; details: string[] } {
  return describeJobProgress(snapshotProgress(step.progress), i18n);
}

/**
 * A line on what a running or retrying workflow step is doing: a running
 * step's progress details or, when it reports none, how long it has run; a
 * retrying step's countdown to its next attempt. Renders nothing otherwise.
 */
export function StepActivity({
  step,
  className,
}: {
  step: DagStep;
  className?: string;
}) {
  const { i18n } = useLingui();
  const status = stepDisplayStatus(step);
  let activity: ReactNode = null;
  if (status === 'RETRYING' && step.retry_after != null) {
    activity = <RetryCountdown retryAt={step.retry_after} />;
  } else if (status === 'RUNNING') {
    const { details } = stepProgress(step, i18n);
    if (details.length > 0) {
      activity = details.join(' · ');
    } else if (step.started_at != null) {
      activity = <RunningFor startedAt={step.started_at} />;
    }
  }
  return activity == null ? null : <p className={className}>{activity}</p>;
}

/** Holds the clock of a running step's elapsed time, so only it re-renders. */
function RunningFor({ startedAt }: { startedAt: number }) {
  const { i18n } = useLingui();
  const now = useNow();
  // Whole seconds: the duration format shows milliseconds below a second.
  const elapsed = formatLocalizedDuration(
    Math.max(Math.floor((now - startedAt) / 1000), 1),
    i18n.locale,
  );
  return <Trans>Running for {elapsed}</Trans>;
}
