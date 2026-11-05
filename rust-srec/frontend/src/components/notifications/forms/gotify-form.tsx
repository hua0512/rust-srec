import {
  FormControl,
  FormField,
  FormItem,
  FormMessage,
} from '@/components/ui/form';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { msg } from '@lingui/core/macro';
import { Globe, KeyRound } from 'lucide-react';
import { useFormContext } from 'react-hook-form';
import { IconInput } from '@/components/ui/icon-input';
import {
  ChannelEnabledField,
  MinPriorityField,
  ChannelLocaleField,
  ChannelTimeoutField,
} from './channel-delivery-fields';
import {
  CONFIG_INPUT,
  ConfigFieldLabel,
} from '@/components/config/shared/config-field';

export function GotifyForm() {
  const { i18n } = useLingui();
  const form = useFormContext();

  return (
    <div className="space-y-4">
      <FormField
        control={form.control}
        name="settings.server_url"
        render={({ field }) => (
          <FormItem className="space-y-2">
            <ConfigFieldLabel>
              <Trans>Server URL</Trans>
            </ConfigFieldLabel>
            <FormControl>
              <IconInput
                icon={Globe}
                placeholder="https://gotify.example.com"
                className={CONFIG_INPUT}
                {...field}
              />
            </FormControl>
            <FormMessage />
          </FormItem>
        )}
      />
      <FormField
        control={form.control}
        name="settings.app_token"
        render={({ field }) => (
          <FormItem className="space-y-2">
            <ConfigFieldLabel>
              <Trans>App Token</Trans>
            </ConfigFieldLabel>
            <FormControl>
              <IconInput
                icon={KeyRound}
                type="password"
                placeholder={i18n._(msg`Gotify application token`)}
                className={CONFIG_INPUT}
                {...field}
              />
            </FormControl>
            <FormMessage />
          </FormItem>
        )}
      />
      <div className="grid items-start gap-4 pt-2 sm:grid-cols-2">
        <MinPriorityField />
        <ChannelLocaleField />
        <ChannelTimeoutField />
      </div>
      <ChannelEnabledField />
    </div>
  );
}
