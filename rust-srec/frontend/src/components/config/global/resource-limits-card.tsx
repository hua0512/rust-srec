import { memo } from 'react';
import { SettingsCard } from '../settings-card';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from '@/components/ui/form';
import { InputWithUnit } from '@/components/ui/input-with-unit';
import { HardDrive } from 'lucide-react';
import { Trans } from '@lingui/react/macro';

export const ResourceLimitsCard = memo(() => {
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
            <FormItem>
              <FormLabel>
                <Trans>Min Segment Size</Trans>
              </FormLabel>
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
            <FormItem>
              <FormLabel>
                <Trans>Max Duration</Trans>
              </FormLabel>
              <FormControl>
                <InputWithUnit
                  unitType="duration"
                  value={field.value}
                  onChange={field.onChange}
                  placeholder="Unlimited"
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
            <FormItem>
              <FormLabel>
                <Trans>Max Part Size</Trans>
              </FormLabel>
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
