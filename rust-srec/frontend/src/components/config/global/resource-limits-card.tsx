import { memo } from 'react';
import { Control } from 'react-hook-form';
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

interface ResourceLimitsCardProps {
  control: Control<any>;
}

export const ResourceLimitsCard = memo(
  ({ control }: ResourceLimitsCardProps) => {
    return (
      <SettingsCard
        title={<Trans>Resource Limits</Trans>}
        description={
          <Trans>Size and duration constraints for recordings.</Trans>
        }
        icon={HardDrive}
        iconColor="text-orange-500"
        iconBgColor="bg-orange-500/10"
      >
        <div className="space-y-6">
          <FormField
            control={control}
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
            control={control}
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
            control={control}
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
  },
);

ResourceLimitsCard.displayName = 'ResourceLimitsCard';
