import { Link } from '@tanstack/react-router';
import { useQuery } from '@tanstack/react-query';
import { HardDrive } from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { msg, plural, t } from '@lingui/core/macro';
import type { I18n } from '@lingui/core';
import { useLingui } from '@lingui/react';

import type { StalledOperation } from '@/api/schemas';
import { stalledOperationsQueryOptions } from '@/api/stalled-operations';
import { HeaderIndicator } from '@/components/header-indicator';
import { getProcessorDefinition } from '@/components/pipeline/presets/processors/registry';
import { formatDuration } from '@/lib/format';

export function stalledOperationsLabel(count: number, i18n: I18n) {
  return t(i18n)`${plural(count, {
    one: '# file operation is not responding',
    other: '# file operations are not responding',
  })}`;
}

/** The job type's display name, as the job pages show it. */
export function stalledOperationTitle(operation: StalledOperation, i18n: I18n) {
  const definition = getProcessorDefinition(operation.job_type);
  return definition
    ? i18n._(definition.label)
    : operation.job_type.replace(/_/g, ' ');
}

/** How long the operation has outlived its job, e.g. "Not responding for 2m 5s". */
export function stalledForLabel(operation: StalledOperation, i18n: I18n) {
  const duration = formatDuration(operation.stalled_secs, { decimals: 0 });
  return i18n._(msg`Not responding for ${duration}`);
}

function inputsLabel(first: string, others: number, i18n: I18n) {
  return i18n._(msg`${first} and ${others} more`);
}

/**
 * Header entry for jobs that already ended while a file operation they started
 * keeps its worker, usually because a disk or network share stopped
 * responding. Renders nothing while every worker is free to take new jobs.
 */
export function StalledOperationsIndicator() {
  const { i18n } = useLingui();
  const { data } = useQuery({
    ...stalledOperationsQueryOptions,
    enabled: typeof window !== 'undefined',
    retry: false,
  });
  const operations = data ?? [];

  return (
    <HeaderIndicator
      count={operations.length}
      label={stalledOperationsLabel(operations.length, i18n)}
      icon={HardDrive}
      tone="orange"
      description={
        <Trans>
          These jobs have ended, but the disk or network share they were using
          stopped responding. Each one keeps its worker until the file system
          answers, so other file jobs may wait in the queue.
        </Trans>
      }
    >
      {(close) => (
        <ul className="py-1">
          {operations.map((operation) => (
            <li key={operation.job_id}>
              <Link
                to="/pipeline/jobs/$jobId"
                params={{ jobId: operation.job_id }}
                onClick={close}
                className="flex min-w-0 flex-col px-4 py-2 hover:bg-accent/60"
              >
                <span className="flex items-center gap-2">
                  <span className="min-w-0 truncate text-sm font-medium">
                    {stalledOperationTitle(operation, i18n)}
                  </span>
                  <span className="ml-auto shrink-0 text-xs font-medium text-orange-600 dark:text-orange-400">
                    {stalledForLabel(operation, i18n)}
                  </span>
                </span>
                {operation.input_file_name && (
                  <span className="mt-0.5 truncate text-[11px] text-muted-foreground">
                    {operation.input_count > 1
                      ? inputsLabel(
                          operation.input_file_name,
                          operation.input_count - 1,
                          i18n,
                        )
                      : operation.input_file_name}
                  </span>
                )}
              </Link>
            </li>
          ))}
        </ul>
      )}
    </HeaderIndicator>
  );
}
