import {
  FormControl,
  FormField,
  FormItem,
  FormMessage,
  FormDescription,
} from '@/components/ui/form';
import { TagInput } from '@/components/ui/tag-input';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { msg } from '@lingui/core/macro';
import { Globe, Hash, User, Shield, Mail } from 'lucide-react';
import { useFormContext } from 'react-hook-form';
import { IconInput } from '@/components/ui/icon-input';
import {
  ChannelEnabledField,
  MinPriorityField,
} from './channel-delivery-fields';
import { SwitchCard } from '@/components/ui/switch-card';
import {
  CONFIG_DESCRIPTION,
  CONFIG_INPUT,
  ConfigFieldLabel,
} from '@/components/config/shared/config-field';

export function EmailForm() {
  const { i18n } = useLingui();
  const form = useFormContext();

  return (
    <div className="space-y-4">
      <div className="grid grid-cols-3 gap-4">
        <div className="col-span-2">
          <FormField
            control={form.control}
            name="settings.smtp_host"
            render={({ field }) => (
              <FormItem className="space-y-2">
                <ConfigFieldLabel>
                  <Trans>SMTP Host</Trans>
                </ConfigFieldLabel>
                <FormControl>
                  <IconInput
                    icon={Globe}
                    placeholder={i18n._(msg`smtp.gmail.com`)}
                    className={CONFIG_INPUT}
                    {...field}
                  />
                </FormControl>
                <FormMessage />
              </FormItem>
            )}
          />
        </div>
        <FormField
          control={form.control}
          name="settings.smtp_port"
          render={({ field }) => (
            <FormItem className="space-y-2">
              <ConfigFieldLabel>
                <Trans>Port</Trans>
              </ConfigFieldLabel>
              <FormControl>
                <IconInput
                  icon={Hash}
                  type="number"
                  placeholder={i18n._(msg`587`)}
                  className={CONFIG_INPUT}
                  {...field}
                  onChange={(e) => field.onChange(e.target.valueAsNumber)}
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
      </div>

      <div className="grid grid-cols-2 gap-4">
        <FormField
          control={form.control}
          name="settings.username"
          render={({ field }) => (
            <FormItem className="space-y-2">
              <ConfigFieldLabel>
                <Trans>Username</Trans>
              </ConfigFieldLabel>
              <FormControl>
                <IconInput
                  icon={User}
                  placeholder={i18n._(msg`Username`)}
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
          name="settings.password"
          render={({ field }) => (
            <FormItem className="space-y-2">
              <ConfigFieldLabel>
                <Trans>Password</Trans>
              </ConfigFieldLabel>
              <FormControl>
                <IconInput
                  icon={Shield}
                  type="password"
                  placeholder={i18n._(msg`Password`)}
                  className={CONFIG_INPUT}
                  {...field}
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
      </div>

      <FormField
        control={form.control}
        name="settings.from_address"
        render={({ field }) => (
          <FormItem className="space-y-2">
            <ConfigFieldLabel>
              <Trans>From Address</Trans>
            </ConfigFieldLabel>
            <FormControl>
              <IconInput
                icon={Mail}
                placeholder={i18n._(msg`notifier@example.com`)}
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
        name="settings.to_addresses"
        render={({ field }) => (
          <FormItem className="space-y-2">
            <ConfigFieldLabel>
              <Trans>To Addresses</Trans>
            </ConfigFieldLabel>
            <FormControl>
              <TagInput
                {...field}
                value={field.value || []}
                onChange={field.onChange}
                placeholder={i18n._(msg`Add email and press Enter`)}
                className={CONFIG_INPUT}
              />
            </FormControl>
            <FormDescription className={CONFIG_DESCRIPTION}>
              <Trans>Press Enter to add recipient</Trans>
            </FormDescription>
            <FormMessage />
          </FormItem>
        )}
      />

      <MinPriorityField />
      <div className="grid gap-4 sm:grid-cols-2">
        <FormField
          control={form.control}
          name="settings.use_tls"
          render={({ field }) => (
            <SwitchCard
              label={<Trans>Use TLS</Trans>}
              checked={field.value}
              onCheckedChange={field.onChange}
              className="h-full"
            />
          )}
        />
        <ChannelEnabledField />
      </div>
    </div>
  );
}
