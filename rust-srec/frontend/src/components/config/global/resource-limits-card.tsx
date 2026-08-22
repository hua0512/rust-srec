import { memo } from 'react';
import { SettingsCard } from '../settings-card';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormMessage,
} from '@/components/ui/form';
import { InputWithUnit } from '@/components/ui/input-with-unit';
import { HardDrive } from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { ConfigFieldLabel } from '@/components/config/shared/config-field';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';

export const ResourceLimitsCard = memo(() => {
  const { i18n } = useLingui();
  return (
    <SettingsCard
      title={<Trans>Resource Limits</Trans>}
      description={<Trans>Size and duration constraints for recordings.</Trans>}
      icon={HardDrive}
      iconColor="text-orange-500"
      iconBgColor="bg-orange-500/10"
    >
      <div className="space-y-6">
        <FormField
          name="min_segment_size_bytes"
          render={({ field }) => (
            <FormItem className="space-y-2">
              <ConfigFieldLabel>
                <Trans>Min Segment Size</Trans>
              </ConfigFieldLabel>
              <FormControl>
                <InputWithUnit
                  unitType="size"
                  value={field.value}
                  onChange={field.onChange}
                  placeholder="0"
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
        <FormField
          name="max_download_duration_secs"
          render={({ field }) => (
            <FormItem className="space-y-2">
              <ConfigFieldLabel>
                <Trans>Max Duration</Trans>
              </ConfigFieldLabel>
              <FormControl>
                <InputWithUnit
                  unitType="duration"
                  value={field.value}
                  onChange={field.onChange}
                  placeholder={i18n._(msg`Unlimited`)}
                />
              </FormControl>
              <FormDescription className="text-xs">
                <Trans>0 = Unlimited</Trans>
              </FormDescription>
              <FormMessage />
            </FormItem>
          )}
        />
        <FormField
          name="max_part_size_bytes"
          render={({ field }) => (
            <FormItem className="space-y-2">
              <ConfigFieldLabel>
                <Trans>Max Part Size</Trans>
              </ConfigFieldLabel>
              <FormControl>
                <InputWithUnit
                  unitType="size"
                  value={field.value}
                  onChange={field.onChange}
                  placeholder="0"
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
      </div>
    </SettingsCard>
  );
});

ResourceLimitsCard.displayName = 'ResourceLimitsCard';
