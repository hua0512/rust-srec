import { Trans } from '@lingui/react/macro';
import {
  ProcessorConfigFormProps,
  useProcessorFieldPath,
} from './common-props';
import {
  ProcessorFormShell,
  ProcessorNumberField,
  ProcessorSection,
} from './processor-form-layout';
import { DeleteConfigSchema } from '../processor-schemas';
import { z } from 'zod';
import { RefreshCw } from 'lucide-react';

type DeleteConfig = z.infer<typeof DeleteConfigSchema>;

export function DeleteConfigForm({
  control,
  pathPrefix,
}: ProcessorConfigFormProps) {
  const fieldPath = useProcessorFieldPath<DeleteConfig>(pathPrefix);

  return (
    <ProcessorFormShell>
      <div className="space-y-6">
        <ProcessorSection
          icon={RefreshCw}
          iconClassName="text-red-500"
          title={<Trans>Retry Policy</Trans>}
        >
          <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
            <ProcessorNumberField
              control={control}
              name={fieldPath('max_retries')}
              label={<Trans>Max Retries</Trans>}
              description={<Trans>Retries for locked files</Trans>}
              min={0}
              step={1}
              placeholder="3"
            />
            <ProcessorNumberField
              control={control}
              name={fieldPath('retry_delay_ms')}
              label={<Trans>Retry Delay (ms)</Trans>}
              description={<Trans>Base delay between retries</Trans>}
              min={0}
              step={100}
              placeholder="100"
            />
          </div>
        </ProcessorSection>
      </div>
    </ProcessorFormShell>
  );
}
