import {
  FormControl,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from '@/components/ui/form';
import { Input } from '@/components/ui/input';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { msg } from '@lingui/core/macro';
import { Globe, User } from 'lucide-react';
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

export function DiscordForm() {
  const { i18n } = useLingui();
  const form = useFormContext();

  return (
    <div className="space-y-4 rounded-xl border border-primary/10 bg-primary/5 p-4">
      <FormField
        control={form.control}
        name="settings.webhook_url"
        render={({ field }) => (
          <FormItem>
            <FormLabel>
              <Trans>Webhook URL</Trans>
            </FormLabel>
            <FormControl>
              <IconInput
                icon={Globe}
                placeholder={i18n._(msg`https://discord.com/api/webhooks/...`)}
                className="bg-background/50"
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
            <FormItem>
              <FormLabel>
                <Trans>Username (Optional)</Trans>
              </FormLabel>
              <FormControl>
                <IconInput
                  icon={User}
                  placeholder={i18n._(msg`Bot Name`)}
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
          name="settings.avatar_url"
          render={({ field }) => (
            <FormItem>
              <FormLabel>
                <Trans>Avatar URL (Optional)</Trans>
              </FormLabel>
              <FormControl>
                <Input
                  placeholder={i18n._(msg`https://...`)}
                  {...field}
                  className="bg-background/50"
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
      </div>
      <div className="pt-2 grid grid-cols-2 gap-4">
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
