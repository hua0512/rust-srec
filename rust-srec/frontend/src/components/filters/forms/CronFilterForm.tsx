import { useFormContext } from 'react-hook-form';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormMessage,
} from '@/components/ui/form';
import { Input } from '@/components/ui/input';
import { useLingui } from '@lingui/react';
import { msg } from '@lingui/core/macro';
import { Trans } from '@lingui/react/macro';
import {
  CONFIG_DESCRIPTION,
  ConfigFieldLabel,
} from '@/components/config/shared/config-field';
import { TimezoneField } from './TimezoneField';

export function CronFilterForm() {
  const { i18n } = useLingui();
  const { control } = useFormContext();

  return (
    <div className="space-y-4">
      <FormField
        control={control}
        name="config.expression"
        render={({ field }) => (
          <FormItem className="space-y-2">
            <ConfigFieldLabel>
              <Trans>Cron Expression</Trans>
            </ConfigFieldLabel>
            <FormControl>
              <Input
                placeholder={i18n._(msg`* * * * * *`)}
                {...field}
                className="font-mono"
              />
            </FormControl>
            <FormDescription className={CONFIG_DESCRIPTION}>
              <Trans>
                Standard cron expression (sec min hour day mon dow).
              </Trans>
            </FormDescription>
            <FormMessage />
          </FormItem>
        )}
      />
      <TimezoneField />
    </div>
  );
}
