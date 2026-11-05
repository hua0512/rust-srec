import { InputWithUnit } from '@/components/ui/input-with-unit';
import { Trans } from '@lingui/react/macro';
import {
  FormField,
  FormItem,
  FormControl,
  FormMessage,
  FormDescription,
} from '@/components/ui/form';
import { SwitchCard } from '@/components/ui/switch-card';
import {
  CONFIG_DESCRIPTION,
  ConfigFieldLabel,
} from '@/components/config/shared/config-field';
import {
  ProcessorConfigFormProps,
  useProcessorFieldPath,
} from './common-props';
import {
  ProcessorFormShell,
  ProcessorNumberField,
  ProcessorSection,
} from './processor-form-layout';
import { ThumbnailConfigSchema } from '../processor-schemas';
import { z } from 'zod';
import { Camera } from 'lucide-react';
import { useWatch } from 'react-hook-form';

type ThumbnailConfig = z.infer<typeof ThumbnailConfigSchema>;

export function ThumbnailConfigForm({
  control,
  pathPrefix,
}: ProcessorConfigFormProps) {
  const fieldPath = useProcessorFieldPath<ThumbnailConfig>(pathPrefix);
  const preserveResolution = useWatch({
    control,
    name: fieldPath('preserve_resolution'),
  });

  return (
    <ProcessorFormShell>
      <ProcessorSection
        icon={Camera}
        iconClassName="text-indigo-500"
        title={<Trans>Capture Settings</Trans>}
      >
        <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
          <FormField
            control={control}
            name={fieldPath('preserve_resolution')}
            render={({ field }) => (
              <SwitchCard
                className="md:col-span-2"
                label={<Trans>Native resolution</Trans>}
                description={
                  <Trans>
                    Keep the original frame size instead of scaling to a width
                  </Trans>
                }
                checked={field.value}
                onCheckedChange={field.onChange}
              />
            )}
          />

          <FormField
            control={control}
            name={fieldPath('timestamp_secs')}
            render={({ field }) => (
              <FormItem>
                <ConfigFieldLabel>
                  <Trans>Timestamp</Trans>
                </ConfigFieldLabel>
                <FormControl>
                  <InputWithUnit
                    unitType="duration"
                    min={0}
                    step={1}
                    value={field.value}
                    onChange={(val) => field.onChange(val ?? 0)}
                    className="bg-background/50"
                  />
                </FormControl>
                <FormDescription className={CONFIG_DESCRIPTION}>
                  <Trans>Time offset to take screenshot from</Trans>
                </FormDescription>
                <FormMessage />
              </FormItem>
            )}
          />

          <ProcessorNumberField
            control={control}
            name={fieldPath('width')}
            label={<Trans>Width</Trans>}
            description={
              preserveResolution ? (
                <Trans>Not used at native resolution</Trans>
              ) : (
                <Trans>Width in pixels (keeps aspect ratio)</Trans>
              )
            }
            min={1}
            step={1}
            placeholder="320"
          />

          <ProcessorNumberField
            control={control}
            name={fieldPath('quality')}
            label={<Trans>Quality (qscale)</Trans>}
            description={<Trans>1 (best) - 31 (worst)</Trans>}
            min={1}
            max={31}
            step={1}
            placeholder="2"
          />
        </div>
      </ProcessorSection>
    </ProcessorFormShell>
  );
}
