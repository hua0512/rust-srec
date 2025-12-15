import { Control } from 'react-hook-form';
import { SettingsCard } from '../settings-card';
import {
  FormControl,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from '@/components/ui/form';
import { Input } from '@/components/ui/input';
import { Separator } from '@/components/ui/separator';
import { Network } from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { ProxyConfigSettings } from '../shared/proxy-settings-card';

interface NetworkSystemCardProps {
  control: Control<any>;
}

export function NetworkSystemCard({ control }: NetworkSystemCardProps) {
  return (
    <SettingsCard
      title={<Trans>Network & System</Trans>}
      description={<Trans>Delays, proxy, and retention policies.</Trans>}
      icon={Network}
      iconColor="text-purple-500"
      iconBgColor="bg-purple-500/10"
    >
      <div className="space-y-6">
        <div className="grid grid-cols-2 gap-6">
          <FormField
            control={control}
            name="streamer_check_delay_ms"
            render={({ field }) => (
              <FormItem>
                <FormLabel>
                  <Trans>Streamer Check (ms)</Trans>
                </FormLabel>
                <FormControl>
                  <Input
                    type="number"
                    {...field}
                    onChange={(e) => field.onChange(Number(e.target.value))}
                  />
                </FormControl>
                <FormMessage />
              </FormItem>
            )}
          />
          <FormField
            control={control}
            name="job_history_retention_days"
            render={({ field }) => (
              <FormItem>
                <FormLabel>
                  <Trans>History Retention (Days)</Trans>
                </FormLabel>
                <FormControl>
                  <Input
                    type="number"
                    {...field}
                    onChange={(e) => field.onChange(Number(e.target.value))}
                  />
                </FormControl>
                <FormMessage />
              </FormItem>
            )}
          />
          <FormField
            control={control}
            name="session_gap_time_secs"
            render={({ field }) => (
              <FormItem>
                <FormLabel>
                  <Trans>Session Gap (seconds)</Trans>
                </FormLabel>
                <FormControl>
                  <Input
                    type="number"
                    {...field}
                    onChange={(e) => field.onChange(Number(e.target.value))}
                  />
                </FormControl>
                <FormMessage />
              </FormItem>
            )}
          />
        </div>
        <div className="grid grid-cols-2 gap-6">
          <FormField
            control={control}
            name="offline_check_delay_ms"
            render={({ field }) => (
              <FormItem>
                <FormLabel>
                  <Trans>Offline Check (ms)</Trans>
                </FormLabel>
                <FormControl>
                  <Input
                    type="number"
                    {...field}
                    onChange={(e) => field.onChange(Number(e.target.value))}
                  />
                </FormControl>
                <FormMessage />
              </FormItem>
            )}
          />
          <FormField
            control={control}
            name="offline_check_count"
            render={({ field }) => (
              <FormItem>
                <FormLabel>
                  <Trans>Offline Check Count</Trans>
                </FormLabel>
                <FormControl>
                  <Input
                    type="number"
                    {...field}
                    onChange={(e) => field.onChange(Number(e.target.value))}
                  />
                </FormControl>
                <FormMessage />
              </FormItem>
            )}
          />
        </div>
        <Separator />
        <FormField
          control={control}
          name="proxy_config"
          render={({ field }) => (
            <FormItem>
              <FormLabel>
                <Trans>Proxy Configuration</Trans>
              </FormLabel>
              <FormControl>
                <ProxyConfigSettings
                  value={field.value}
                  onChange={field.onChange}
                  outputFormat="object"
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
      </div>
    </SettingsCard>
  );
}
