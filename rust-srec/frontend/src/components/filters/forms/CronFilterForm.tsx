import { useFormContext } from 'react-hook-form';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from '@/components/ui/form';
import { Input } from '@/components/ui/input';
import { Trans } from '@lingui/macro';

export function CronFilterForm() {
  const { control } = useFormContext();

  return (
    <div className="space-y-4">
      <FormField
        control={control}
        name="config.expression"
        render={({ field }) => (
          <FormItem>
            <FormLabel>
              <Trans>Cron Expression</Trans>
            </FormLabel>
            <FormControl>
              <Input
                placeholder="* * * * * *"
                {...field}
                className="font-mono"
              />
            </FormControl>
            <FormDescription>
              <Trans>
                Standard cron expression (sec min hour day mon year).
              </Trans>
            </FormDescription>
            <FormMessage />
          </FormItem>
        )}
      />
      <FormField
        control={control}
        name="config.timezone"
        render={({ field }) => (
          <FormItem>
            <FormLabel>
              <Trans>Timezone</Trans>
            </FormLabel>
            <FormControl>
              <Input placeholder="UTC" {...field} />
            </FormControl>
            <FormDescription>
              <Trans>IANA Timezone (e.g. Asia/Shanghai, UTC).</Trans>
            </FormDescription>
            <FormMessage />
          </FormItem>
        )}
      />
    </div>
  );
}
