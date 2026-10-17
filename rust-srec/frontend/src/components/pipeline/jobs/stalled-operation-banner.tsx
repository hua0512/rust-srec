import { useQuery } from '@tanstack/react-query';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { HardDrive } from 'lucide-react';

import { stalledOperationsQueryOptions } from '@/api/stalled-operations';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { stalledForLabel } from '@/components/stalled-operations-indicator';

/**
 * Explains a finished job that still holds a worker: its status alone reads as
 * done while a file operation it started waits on the file system.
 */
export function StalledOperationBanner({
  jobId,
  enabled,
}: {
  jobId: string;
  /** Only an ended job can be stalled; skip the query otherwise. */
  enabled: boolean;
}) {
  const { i18n } = useLingui();
  const { data } = useQuery({
    ...stalledOperationsQueryOptions,
    enabled: enabled && typeof window !== 'undefined',
    retry: false,
  });
  const operation = data?.find((candidate) => candidate.job_id === jobId);
  if (!operation) return null;

  return (
    <Card
      role="status"
      className="border-orange-500/30 bg-orange-500/5 backdrop-blur-sm shadow-sm overflow-hidden"
    >
      <div className="h-1 w-full bg-orange-500/40" />
      <CardHeader className="pb-2">
        <CardTitle className="flex items-center gap-2 text-base text-orange-600 dark:text-orange-400">
          <HardDrive className="h-5 w-5" />
          <Trans>Still holding a worker</Trans>
          <span className="ml-auto text-xs font-medium">
            {stalledForLabel(operation, i18n)}
          </span>
        </CardTitle>
      </CardHeader>
      <CardContent className="text-sm text-muted-foreground">
        <Trans>
          This job has ended, but a file operation it started has not returned.
          The disk or network share it uses stopped responding, and the worker
          stays reserved until it answers. Check that the destination is
          reachable.
        </Trans>
      </CardContent>
    </Card>
  );
}
