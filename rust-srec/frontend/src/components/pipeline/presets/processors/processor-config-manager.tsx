import { Control, UseFormRegister } from 'react-hook-form';
import { Trans } from '@lingui/react/macro';
import { Suspense } from 'react';
import { getProcessorDefinition } from './registry';
import { Skeleton } from '@/components/ui/skeleton';

interface ProcessorConfigManagerProps {
  processorType: string;
  control: Control<any>;
  register?: UseFormRegister<any>;
  pathPrefix?: string;
}

export function ProcessorConfigManager({
  processorType,
  control,
  pathPrefix,
}: ProcessorConfigManagerProps) {
  const definition = getProcessorDefinition(processorType);

  if (!definition) {
    return (
      <div className="text-muted-foreground italic">
        <Trans>No configuration available for this processor.</Trans>
      </div>
    );
  }

  const ConfigForm = definition.component;

  return (
    <Suspense
      fallback={
        <div className="space-y-4">
          <Skeleton className="h-20 w-full rounded-xl" />
          <Skeleton className="h-40 w-full rounded-xl" />
        </div>
      }
    >
      <ConfigForm control={control} pathPrefix={pathPrefix} />
    </Suspense>
  );
}
