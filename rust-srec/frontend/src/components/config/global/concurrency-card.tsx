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
import { Input } from '@/components/ui/input';
import { Separator } from '@/components/ui/separator';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { Cpu } from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { useQuery } from '@tanstack/react-query';
import { listEngines } from '@/server/functions';

export interface ConcurrencyCardProps {
  control: Control<any>;
}

export const ConcurrencyCard = memo(({ control }: ConcurrencyCardProps) => {
  const { i18n } = useLingui();
  const { data: enginesData, isLoading: enginesLoading } = useQuery({
    queryKey: ['engines'],
    queryFn: () => listEngines(),
  });

  const engines = enginesData || [];

  return (
    <SettingsCard
      title={<Trans>Concurrency & Performance</Trans>}
      description={<Trans>Job limits and engine settings.</Trans>}
      icon={Cpu}
      iconColor="text-green-500"
      iconBgColor="bg-green-500/10"
    >
      <div className="space-y-6">
        <div className="grid grid-cols-2 gap-6">
          <FormField
            control={control}
            name="max_concurrent_downloads"
            render={({ field }) => (
              <FormItem>
                <FormLabel>
                  <Trans>Max Downloads</Trans>
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
            name="max_concurrent_uploads"
            render={({ field }) => (
              <FormItem>
                <FormLabel>
                  <Trans>Max Uploads</Trans>
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
            name="max_concurrent_cpu_jobs"
            render={({ field }) => (
              <FormItem>
                <FormLabel>
                  <Trans>Max CPU Jobs</Trans>
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
            name="max_concurrent_io_jobs"
            render={({ field }) => (
              <FormItem>
                <FormLabel>
                  <Trans>Max IO Jobs</Trans>
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
          name="default_download_engine"
          render={({ field }) => (
            <FormItem>
              <FormLabel>
                <Trans>Default Engine</Trans>
              </FormLabel>
              <Select
                onValueChange={field.onChange}
                value={field.value}
                disabled={enginesLoading}
              >
                <FormControl>
                  <SelectTrigger>
                    <SelectValue
                      placeholder={i18n._(msg`Select a default engine`)}
                    />
                  </SelectTrigger>
                </FormControl>
                <SelectContent>
                  {engines?.map((engine) => (
                    <SelectItem key={engine.id} value={engine.name}>
                      {engine.name} ({engine.engine_type})
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              <FormDescription>
                <Trans>
                  Engine used for downloads when not specified by
                  platform/streamer.
                </Trans>
              </FormDescription>
              <FormMessage />
            </FormItem>
          )}
        />
      </div>
    </SettingsCard>
  );
});

ConcurrencyCard.displayName = 'ConcurrencyCard';
