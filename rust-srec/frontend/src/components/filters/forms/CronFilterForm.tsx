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
import { useLingui } from '@lingui/react';
import { msg } from '@lingui/core/macro';
import { Trans } from '@lingui/react/macro';

export function CronFilterForm() {
  const { i18n } = useLingui();
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
                placeholder={i18n._(msg`* * * * * *`)}
                {...field}
                className="font-mono"
              />
            </FormControl>
            <FormDescription>
              <Trans>
                Standard cron expression (sec min hour day mon dow).
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
              <Input placeholder={i18n._(msg`UTC`)} {...field} />
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
