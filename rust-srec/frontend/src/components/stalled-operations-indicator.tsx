import { useState } from 'react';
import { Link } from '@tanstack/react-router';
import { queryOptions, useQuery } from '@tanstack/react-query';
import { HardDrive } from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { msg, plural, t } from '@lingui/core/macro';
import type { I18n } from '@lingui/core';
import { useLingui } from '@lingui/react';

import type { StalledOperation } from '@/api/schemas';
import { getProcessorDefinition } from '@/components/pipeline/presets/processors/registry';
import { NotificationBadge } from '@/components/ui/notification-badge';
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from '@/components/ui/popover';
import {
  HEADER_BADGE_DOT,
  HEADER_POPOVER,
  HEADER_TRIGGER_EXIT_MS,
  headerTriggerClass,
} from '@/components/header-indicator';
import { usePresence } from '@/hooks/use-presence';
import { formatDuration } from '@/lib/format';
import { cn } from '@/lib/utils';
import { listStalledOperations } from '@/server/functions/pipeline';

/**
 * Ended jobs whose file operation still holds a worker. The list lives in
 * backend memory only, so polling it is cheap; it changes when a disk or share
 * stops or resumes answering, which no live event reports.
 */
export const stalledOperationsQueryOptions = queryOptions({
  queryKey: ['pipeline', 'stalled-operations'],
  queryFn: () => listStalledOperations(),
  refetchInterval: 15_000,
  staleTime: 10_000,
});

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
  const [open, setOpen] = useState(false);
  const { data } = useQuery({
    ...stalledOperationsQueryOptions,
    enabled: typeof window !== 'undefined',
    retry: false,
  });
  const operations = data ?? [];
  const hasContent = operations.length > 0;
  const { mounted, exiting } = usePresence(hasContent, HEADER_TRIGGER_EXIT_MS);

  if (!mounted) return null;

  const label = stalledOperationsLabel(operations.length, i18n);

  return (
    <Popover open={open && hasContent} onOpenChange={setOpen}>
      <PopoverTrigger
        aria-label={label}
        title={label}
        className={cn(
          headerTriggerClass(exiting),
          'text-orange-600 hover:text-orange-600 dark:text-orange-400 dark:hover:text-orange-400',
        )}
      >
        <HardDrive className="size-4" />
        <NotificationBadge
          open={hasContent}
          entering={hasContent}
          className="-top-0.5 -right-0.5"
          dotClassName={cn(HEADER_BADGE_DOT, 'bg-orange-500')}
        >
          {hasContent ? operations.length : undefined}
        </NotificationBadge>
      </PopoverTrigger>
      <PopoverContent align="end" sideOffset={8} className={HEADER_POPOVER}>
        <div className="space-y-1 px-4 py-3">
          <p className="text-sm font-semibold">{label}</p>
          <p className="text-xs text-muted-foreground">
            <Trans>
              These jobs have ended, but the disk or network share they were
              using stopped responding. Each one keeps its worker until the file
              system answers, so other file jobs may wait in the queue.
            </Trans>
          </p>
        </div>
        <ul className="max-h-80 overflow-y-auto border-t border-border/60 py-1">
          {operations.map((operation) => (
            <li key={operation.job_id}>
              <Link
                to="/pipeline/jobs/$jobId"
                params={{ jobId: operation.job_id }}
                onClick={() => setOpen(false)}
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
      </PopoverContent>
    </Popover>
  );
}
