import {
  FormControl,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
  FormDescription,
} from '@/components/ui/form';
import { TagInput } from '@/components/ui/tag-input';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { msg } from '@lingui/core/macro';
import { Globe, Hash, User, Shield, Mail } from 'lucide-react';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { useFormContext } from 'react-hook-form';
import { IconInput } from '@/components/ui/icon-input';
import { SwitchCard } from '@/components/ui/switch-card';

export function EmailForm() {
  const { i18n } = useLingui();
  const form = useFormContext();

  return (
    <div className="space-y-4 rounded-xl border border-primary/10 bg-primary/5 p-4">
      <div className="grid grid-cols-3 gap-4">
        <div className="col-span-2">
          <FormField
            control={form.control}
            name="settings.smtp_host"
            render={({ field }) => (
              <FormItem>
                <FormLabel>
                  <Trans>SMTP Host</Trans>
                </FormLabel>
                <FormControl>
                  <IconInput
                    icon={Globe}
                    placeholder={i18n._(msg`smtp.gmail.com`)}
                    className="bg-background/50"
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
            <FormItem>
              <FormLabel>
                <Trans>Port</Trans>
              </FormLabel>
              <FormControl>
                <IconInput
                  icon={Hash}
                  type="number"
                  placeholder={i18n._(msg`587`)}
                  className="bg-background/50"
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
            <FormItem>
              <FormLabel>
                <Trans>Username</Trans>
              </FormLabel>
              <FormControl>
                <IconInput
                  icon={User}
                  placeholder={i18n._(msg`Username`)}
                  className="bg-background/50"
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
            <FormItem>
              <FormLabel>
                <Trans>Password</Trans>
              </FormLabel>
              <FormControl>
                <IconInput
                  icon={Shield}
                  type="password"
                  placeholder={i18n._(msg`Password`)}
                  className="bg-background/50"
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
          <FormItem>
            <FormLabel>
              <Trans>From Address</Trans>
            </FormLabel>
            <FormControl>
              <IconInput
                icon={Mail}
                placeholder={i18n._(msg`notifier@example.com`)}
                className="bg-background/50"
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
          <FormItem>
            <FormLabel>
              <Trans>To Addresses</Trans>
            </FormLabel>
            <FormControl>
              <TagInput
                {...field}
                value={field.value || []}
                onChange={field.onChange}
                placeholder={i18n._(msg`Add email and press Enter`)}
                className="bg-background/50"
              />
            </FormControl>
            <FormDescription>
              <Trans>Press Enter to add recipient</Trans>
            </FormDescription>
            <FormMessage />
          </FormItem>
        )}
      />

      <div className="grid grid-cols-3 gap-4">
        <FormField
          control={form.control}
          name="settings.min_priority"
          render={({ field }) => (
            <FormItem>
              <FormLabel>
                <Trans>Min Priority</Trans>
              </FormLabel>
              <Select onValueChange={field.onChange} defaultValue={field.value}>
                <FormControl>
                  <SelectTrigger className="bg-background/50">
                    <SelectValue />
                  </SelectTrigger>
                </FormControl>
                <SelectContent>
                  <SelectItem value="Low">
                    <Trans>Low</Trans>
                  </SelectItem>
                  <SelectItem value="Normal">
                    <Trans>Normal</Trans>
                  </SelectItem>
                  <SelectItem value="High">
                    <Trans>High</Trans>
                  </SelectItem>
                  <SelectItem value="Critical">
                    <Trans>Critical</Trans>
                  </SelectItem>
                </SelectContent>
              </Select>
              <FormMessage />
            </FormItem>
          )}
        />
        <FormField
          control={form.control}
          name="settings.use_tls"
          render={({ field }) => (
            <SwitchCard
              label={<Trans>Use TLS</Trans>}
              checked={field.value}
              onCheckedChange={field.onChange}
              className="border-primary/10 bg-background/50 h-full"
            />
          )}
        />
        <FormField
          control={form.control}
          name="settings.enabled"
          render={({ field }) => (
            <SwitchCard
              label={<Trans>Enabled</Trans>}
              checked={field.value}
              onCheckedChange={field.onChange}
              className="border-primary/10 bg-background/50 h-full"
            />
          )}
        />
      </div>
    </div>
  );
}
