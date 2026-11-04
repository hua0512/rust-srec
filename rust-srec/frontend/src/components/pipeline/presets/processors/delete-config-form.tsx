import { Trans } from '@lingui/react/macro';
import {
  FormField,
  FormItem,
  FormLabel,
  FormControl,
  FormMessage,
  FormDescription,
} from '@/components/ui/form';
import {
  ProcessorConfigFormProps,
  useProcessorFieldPath,
} from './common-props';
import { ProcessorFormShell, ProcessorSection } from './processor-form-layout';
import { DeleteConfigSchema } from '../processor-schemas';
import { z } from 'zod';
import { RefreshCw } from 'lucide-react';
import { NumberInput } from '@/components/ui/number-input';

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
            <FormField
              control={control}
              name={fieldPath('max_retries')}
              render={({ field }) => (
                <FormItem>
                  <FormLabel className="text-xs text-muted-foreground ml-1">
                    <Trans>Max Retries</Trans>
                  </FormLabel>
                  <FormControl>
                    <NumberInput
                      field={field}
                      className="h-11 bg-background/50 border-border/50 focus:bg-background rounded-lg font-mono text-sm"
                      min={0}
                      step={1}
                      placeholder="3"
                    />
                  </FormControl>
                  <FormDescription className="text-[11px] ml-1">
                    <Trans>Retries for locked files</Trans>
                  </FormDescription>
                  <FormMessage />
                </FormItem>
              )}
            />

            <FormField
              control={control}
              name={fieldPath('retry_delay_ms')}
              render={({ field }) => (
                <FormItem>
                  <FormLabel className="text-xs text-muted-foreground ml-1">
                    <Trans>Retry Delay (ms)</Trans>
                  </FormLabel>
                  <FormControl>
                    <NumberInput
                      field={field}
                      className="h-11 bg-background/50 border-border/50 focus:bg-background rounded-lg font-mono text-sm"
                      min={0}
                      step={100}
                      placeholder="100"
                    />
                  </FormControl>
                  <FormDescription className="text-[11px] ml-1">
                    <Trans>Base delay between retries</Trans>
                  </FormDescription>
                  <FormMessage />
                </FormItem>
              )}
            />
          </div>
        </ProcessorSection>
      </div>
    </ProcessorFormShell>
  );
}
