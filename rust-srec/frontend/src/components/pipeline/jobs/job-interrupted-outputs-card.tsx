import { useQuery } from '@tanstack/react-query';
import { FileWarning } from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';

import { interruptedOutputsQueryOptions } from '@/api/interrupted-outputs';
import {
  DeleteAllInterruptedOutputs,
  InterruptedOutputList,
  interruptedOutputsLabel,
} from '@/components/pipeline/interrupted-output-list';
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/components/ui/card';

/** Partial files an interrupted attempt of this job left behind, if any. */
export function JobInterruptedOutputsCard({ jobId }: { jobId: string }) {
  const { i18n } = useLingui();
  const { data } = useQuery({
    ...interruptedOutputsQueryOptions(jobId),
    retry: false,
  });
  const outputs = data?.outputs ?? [];
  if (outputs.length === 0) return null;
  const totalBytes = data?.total_size_bytes ?? 0;

  return (
    <Card className="overflow-hidden border-amber-500/30 bg-amber-500/5 shadow-sm backdrop-blur-sm">
      <CardHeader className="pb-2">
        <CardTitle className="flex items-center gap-2 text-base text-amber-700 dark:text-amber-400">
          <FileWarning className="h-5 w-5" />
          {interruptedOutputsLabel(outputs.length, totalBytes, i18n)}
        </CardTitle>
        <CardDescription>
          <Trans>
            A crash or forced exit stopped an earlier run of this job before it
            could clean up. These files are unfinished and safe to delete.
          </Trans>
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-2 px-0 pb-3">
        <InterruptedOutputList outputs={outputs} showJobLink={false} />
        {outputs.length > 1 && (
          <div className="px-4">
            <DeleteAllInterruptedOutputs
              outputs={outputs}
              totalBytes={totalBytes}
            />
          </div>
        )}
      </CardContent>
    </Card>
  );
}
