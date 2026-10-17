import { useQuery } from '@tanstack/react-query';
import { FileWarning } from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';

import { interruptedOutputsQueryOptions } from '@/api/interrupted-outputs';
import { HeaderIndicator } from '@/components/header-indicator';
import {
  DeleteAllInterruptedOutputs,
  InterruptedOutputList,
  interruptedOutputsLabel,
} from '@/components/pipeline/interrupted-output-list';

/**
 * Header entry for partial temporary files that jobs interrupted by a crash,
 * power loss or forced exit left next to their destinations. Renders nothing
 * while there are none.
 */
export function InterruptedOutputsIndicator() {
  const { i18n } = useLingui();
  const { data } = useQuery({
    ...interruptedOutputsQueryOptions(),
    enabled: typeof window !== 'undefined',
    retry: false,
  });
  const outputs = data?.outputs ?? [];
  const totalBytes = data?.total_size_bytes ?? 0;

  return (
    <HeaderIndicator
      count={outputs.length}
      label={interruptedOutputsLabel(outputs.length, totalBytes, i18n)}
      icon={FileWarning}
      tone="amber"
      description={
        <Trans>
          A crash or forced exit stopped these jobs before they could clean up.
          The files are unfinished and safe to delete.
        </Trans>
      }
      actions={
        <DeleteAllInterruptedOutputs
          outputs={outputs}
          totalBytes={totalBytes}
        />
      }
    >
      {(close) => (
        <InterruptedOutputList outputs={outputs} onNavigate={close} />
      )}
    </HeaderIndicator>
  );
}
