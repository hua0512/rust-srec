import {
  FormControl,
  FormField,
  FormItem,
  FormMessage,
} from '@/components/ui/form';
import { Input } from '@/components/ui/input';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { msg } from '@lingui/core/macro';
import { Globe, User } from 'lucide-react';
import { useFormContext } from 'react-hook-form';
import { ChannelDeliveryFields } from './channel-delivery-fields';
import { IconInput } from '@/components/ui/icon-input';
import {
  CONFIG_INPUT,
  ConfigFieldLabel,
} from '@/components/config/shared/config-field';

export function DiscordForm() {
  const { i18n } = useLingui();
  const form = useFormContext();

  return (
    <div className="space-y-4">
      <FormField
        control={form.control}
        name="settings.webhook_url"
        render={({ field }) => (
          <FormItem className="space-y-2">
            <ConfigFieldLabel>
              <Trans>Webhook URL</Trans>
            </ConfigFieldLabel>
            <FormControl>
              <IconInput
                icon={Globe}
                placeholder={i18n._(msg`https://discord.com/api/webhooks/...`)}
                className={CONFIG_INPUT}
                {...field}
              />
            </FormControl>
            <FormMessage />
          </FormItem>
        )}
      />
      <div className="grid grid-cols-2 gap-4">
        <FormField
          control={form.control}
          name="settings.username"
          render={({ field }) => (
            <FormItem className="space-y-2">
              <ConfigFieldLabel>
                <Trans>Username (Optional)</Trans>
              </ConfigFieldLabel>
              <FormControl>
                <IconInput
                  icon={User}
                  placeholder={i18n._(msg`Bot Name`)}
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
          name="settings.avatar_url"
          render={({ field }) => (
            <FormItem className="space-y-2">
              <ConfigFieldLabel>
                <Trans>Avatar URL (Optional)</Trans>
              </ConfigFieldLabel>
              <FormControl>
                <Input
                  placeholder={i18n._(msg`https://...`)}
                  {...field}
                  className={CONFIG_INPUT}
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
      </div>
      <ChannelDeliveryFields />
    </div>
  );
}
