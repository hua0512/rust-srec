import { Fragment } from 'react';
import { Link } from '@tanstack/react-router';
import { Workflow } from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { msg, plural, t } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { useShallow } from 'zustand/react/shallow';

import { HeaderIndicator } from '@/components/header-indicator';
import { Progress } from '@/components/ui/progress';
import {
  displayName,
  StreamerAvatar,
} from '@/components/upload-status-indicator';
import { useNow } from '@/hooks/use-now';
import { formatLocalizedDuration } from '@/lib/date-utils';
import { describeJobProgress, jobTypeLabel } from '@/lib/job-progress';
import { cn } from '@/lib/utils';
import { useUploadStore, type JobView } from '@/store/uploads';

/**
 * Header entry for pipeline jobs other than uploads, such as remuxing,
 * transcoding, copies and compression, once they have run for 10 seconds.
 * Renders nothing while no such job runs.
 */
export function ProcessingJobsIndicator() {
  const { i18n } = useLingui();
  const jobs = useUploadStore(useShallow((state) => state.getActiveJobs()));
  const count = jobs.length;

  return (
    <HeaderIndicator
      count={count}
      label={t(
        i18n,
      )`${plural(count, { one: '# job in progress', other: '# jobs in progress' })}`}
      icon={Workflow}
      tone="violet"
      description={
        <Trans>Processing steps that have run for more than 10 seconds.</Trans>
      }
    >
      {(close) => <JobList jobs={jobs} onNavigate={close} />}
    </HeaderIndicator>
  );
}

function JobList({
  jobs,
  onNavigate,
}: {
  jobs: JobView[];
  onNavigate: () => void;
}) {
  // Mounted only while the popover is open; keeps elapsed times current.
  const now = useNow();

  return (
    <ul className="py-1">
      {jobs.map((job) => (
        <li key={job.jobId}>
          <JobRow job={job} now={now} onNavigate={onNavigate} />
        </li>
      ))}
    </ul>
  );
}

function JobRow({
  job,
  now,
  onNavigate,
}: {
  job: JobView;
  now: number;
  onNavigate: () => void;
}) {
  const { i18n } = useLingui();
  const name = displayName(job, i18n);
  const { percent, details } = describeJobProgress(job, i18n);
  if (details.length === 0 && job.startedAtMs > 0n) {
    const elapsed = formatLocalizedDuration(
      Math.max(now - Number(job.startedAtMs), 0) / 1000,
      i18n.locale,
    );
    details.push(i18n._(msg`Running for ${elapsed}`));
  }

  return (
    <Link
      to="/pipeline/jobs/$jobId"
      params={{ jobId: job.jobId }}
      onClick={onNavigate}
      className="flex items-center gap-3 px-4 py-2 hover:bg-accent/60"
    >
      <StreamerAvatar upload={job} name={name} />
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-3">
          <span
            className={cn(
              'truncate text-sm font-medium',
              percent != null ? 'w-24 shrink-0' : 'min-w-0',
            )}
          >
            {name}
          </span>
          {percent != null && (
            <>
              <Progress
                value={percent}
                className="h-1 flex-1 bg-violet-500/15"
                indicatorClassName="bg-violet-500"
              />
              <span className="w-9 shrink-0 text-right text-xs font-semibold tabular-nums">
                {`${percent.toFixed(0)}%`}
              </span>
            </>
          )}
        </div>
        <div className="mt-0.5 truncate text-[11px] text-muted-foreground">
          {jobTypeLabel(job.jobType, i18n)}
        </div>
        {/* Wraps only between details, so no value splits across lines. */}
        <div className="text-[11px] text-muted-foreground">
          {details.map((detail, index) => (
            <Fragment key={detail}>
              {index > 0 && ' · '}
              <span className="whitespace-nowrap">{detail}</span>
            </Fragment>
          ))}
        </div>
      </div>
    </Link>
  );
}
